//! The settings a script may name in its `settings` block and its `quick`
//! line: the one list of them, [`TABLE`].
//!
//! A row has our name for the setting (snake case; a setting a script
//! reads as `variant.<name>`, `caps::VARIANT_FIELDS`, keeps that name), a
//! label of ours, the kind of value it takes, where it is stored and the
//! carriers it applies to. Rows are appended, never reordered: a blob
//! names a setting by its index in the table (`ir::Setting`).
//!
//! The rules crate does not depend on `variants`: a row names its store
//! by the record's setting numbers (`variants::record::SettingId`) and the
//! `hx` block's fields (`variants::ext::Ext`), and a test in `variants`
//! holds the table to `record::field`, `schema::applies` and the record
//! clean-up's ranges. See `docs/game/rules-variants.md`, "The settings
//! block", and `docs/game/rules-engine.md`, "The language".

use crate::caps::CARRIERS;

/// A named value of a choice setting: its name in a script, our label
/// and the value the record stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Named {
    pub name: &'static str,
    pub label: &'static str,
    pub value: i32,
}

const fn n(name: &'static str, label: &'static str, value: i32) -> Named {
    Named { name, label, value }
}

/// The kind of value a setting takes, and its range: the values the
/// record's clean-up keeps (`variants::record::check`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `true` or `false`, stored as 1 or 0.
    Bool,
    /// A number in the range.
    Int { lo: i32, hi: i32 },
    /// A time literal in whole seconds (`300s`); the record holds seconds.
    Seconds { lo: i32, hi: i32 },
    /// A value's name from `names`, or a number in the range.
    Choice {
        lo: i32,
        hi: i32,
        names: &'static [Named],
    },
    /// A percent (`gravity = 50`).
    Percent { lo: i32, hi: i32 },
}

impl Kind {
    /// The lowest and highest value.
    pub fn range(self) -> (i32, i32) {
        match self {
            Kind::Bool => (0, 1),
            Kind::Int { lo, hi }
            | Kind::Seconds { lo, hi }
            | Kind::Choice { lo, hi, .. }
            | Kind::Percent { lo, hi } => (lo, hi),
        }
    }

    /// The kind's word, for messages and hovers.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Bool => "bool",
            Kind::Int { .. } => "int",
            Kind::Seconds { .. } => "seconds",
            Kind::Choice { .. } => "choice",
            Kind::Percent { .. } => "percent",
        }
    }

    /// The named values of a choice; none for the other kinds.
    pub fn names(self) -> &'static [Named] {
        match self {
            Kind::Choice { names, .. } => names,
            _ => &[],
        }
    }
}

/// A field of the `hx` block (`variants::ext::Ext`), the extras the game
/// applies (docs/game/game-options.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtField {
    Gravity,
    Damage,
    Shield,
    Health,
    UnlimitedGrenades,
}

/// Where a row's value is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store {
    /// One record setting (`variants::record::SettingId`).
    Setting(u32),
    /// The carrier's own score setting (`GameType::score_setting`,
    /// settings 1 to 9).
    Score,
    /// CTF's setting on `ctf` and Assault's on `assault`, where the two
    /// engines pair their settings.
    Paired { ctf: u32, assault: u32 },
    /// A field of the `hx` block.
    Ext(ExtField),
}

/// One setting a script may name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// The name a script writes.
    pub name: &'static str,
    /// Our label ("Starting Weapon").
    pub label: &'static str,
    pub kind: Kind,
    pub store: Store,
    /// The carriers (engine indices, `caps::CARRIERS`) the row applies to;
    /// empty for every carrier.
    pub carriers: &'static [u8],
    /// 0 means none, and the block takes `none` for it.
    pub zero_is_none: bool,
}

impl Row {
    /// Whether the row applies to the carrier with engine index `carrier`.
    pub fn applies(&self, carrier: u8) -> bool {
        if carrier == crate::SCRIPT_INDEX {
            return self.carriers.is_empty();
        }
        CARRIERS.iter().any(|&(_, e)| e == carrier)
            && (self.carriers.is_empty() || self.carriers.contains(&carrier))
    }

    /// The record setting the row stores on `carrier`: `None` for an `hx`
    /// field and for a carrier the row does not apply to.
    pub fn setting(&self, carrier: u8) -> Option<u32> {
        if !self.applies(carrier) {
            return None;
        }
        match self.store {
            Store::Setting(s) => Some(s),
            Store::Score => score_setting(carrier),
            Store::Paired { ctf, assault } => match carrier {
                1 => Some(ctf),
                9 => Some(assault),
                _ => None,
            },
            Store::Ext(_) => None,
        }
    }

    /// Whether the row is a record setting (not an `hx` field): what a
    /// Quick Options mark may name.
    pub fn is_record(&self) -> bool {
        !matches!(self.store, Store::Ext(_))
    }

    /// The value `none` stands for: a choice's value named `none`, or 0
    /// for a row whose zero means none.
    pub fn none_value(&self) -> Option<i32> {
        self.choice("none").or(self.zero_is_none.then_some(0))
    }

    /// The value of the choice named `name`.
    pub fn choice(&self, name: &str) -> Option<i32> {
        self.kind
            .names()
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.value)
    }

    /// Whether `value` is in the row's range.
    pub fn in_range(&self, value: i32) -> bool {
        let (lo, hi) = self.kind.range();
        (lo..=hi).contains(&value)
    }

    /// The game types the row applies to, in words: "every game type",
    /// "Juggernaut", "Capture the Flag and Assault".
    pub fn carrier_words(&self) -> String {
        if self.carriers.is_empty() {
            return "every game type".into();
        }
        let words: Vec<&str> = self.carriers.iter().map(|&c| carrier_label(c)).collect();
        words.join(" and ")
    }
}

/// The game type's label for an engine index.
pub fn carrier_label(carrier: u8) -> &'static str {
    match carrier {
        1 => "Capture the Flag",
        2 => "Slayer",
        3 => "Oddball",
        4 => "King of the Hill",
        crate::SCRIPT_INDEX => "Script",
        7 => "Juggernaut",
        8 => "Territories",
        9 => "Assault",
        _ => "an unknown game type",
    }
}

/// The setting that holds a carrier's score to win (`GameType::score_setting`).
pub fn score_setting(carrier: u8) -> Option<u32> {
    match carrier {
        1 => Some(1),
        2 | crate::SCRIPT_INDEX => Some(2),
        3 => Some(3),
        4 => Some(4),
        7 => Some(7),
        8 => Some(8),
        9 => Some(9),
        _ => None,
    }
}

// ---- the named values a script writes for an enumerated setting ----

/// Starting and secondary weapons (settings 89 and 94). 1 is no weapon,
/// for the starting weapon one of our added choices (`variants::extra`).
const WEAPONS: &[Named] = &[
    n("map_default", "Map Default", 0),
    n("none", "None", 1),
    n("random", "Random", 2),
    n("battle_rifle", "Battle Rifle", 3),
    n("magnum", "Magnum", 4),
    n("smg", "SMG", 5),
    n("plasma_pistol", "Plasma Pistol", 6),
    n("plasma_rifle", "Plasma Rifle", 7),
    n("rocket_launcher", "Rocket Launcher", 8),
    n("shotgun", "Shotgun", 9),
    n("sniper_rifle", "Sniper Rifle", 10),
    n("brute_shot", "Brute Shot", 11),
    n("needler", "Needler", 12),
    n("carbine", "Carbine", 13),
    n("beam_rifle", "Beam Rifle", 14),
    n("energy_sword", "Energy Sword", 17),
    n("brute_plasma_rifle", "Brute Plasma Rifle", 18),
    n("sentinel_beam", "Sentinel Beam", 19),
];

/// Weapons on map (setting 83).
const WEAPON_SETS: &[Named] = &[
    n("none", "None", 1),
    n("rockets", "Rockets", 2),
    n("shotguns", "Shotguns", 3),
    n("swords", "Swords", 4),
    n("brute_shots", "Brute Shots", 5),
    n("rifles", "Rifles", 11),
    n("sniping", "Sniping", 12),
    n("pistols", "Pistols", 14),
];

/// Shields (setting 19).
const SHIELDS: &[Named] = &[
    n("normal", "Normal", 0),
    n("none", "None", 1),
    n("overshields", "Overshields", 2),
];

/// Light vehicles (settings 76 and 77).
const LIGHT_VEHICLES: &[Named] = &[
    n("warthog", "Warthog", 1),
    n("gauss_warthog", "Gauss Warthog", 2),
    n("ghost", "Ghost", 3),
    n("spectre", "Spectre", 5),
    n("none", "None", 7),
];

/// Heavy vehicle (setting 78).
const HEAVY_VEHICLES: &[Named] = &[n("none", "None", 4)];

/// Flying vehicle (setting 79).
const FLYING_VEHICLES: &[Named] = &[n("banshee", "Banshee", 1), n("none", "None", 4)];

/// Turrets (settings 81 and 82).
const TURRETS: &[Named] = &[
    n("machine_gun", "Machine Gun", 1),
    n("plasma", "Plasma", 4),
    n("none", "None", 6),
];

/// Weapon respawn (setting 87).
const WEAPON_RESPAWN: &[Named] = &[n("double_time", "Double Time", 3)];

/// Respawn modifier (setting 25, `+0xa8`).
const RESPAWN_MODIFIERS: &[Named] = &[
    n("inheritance", "Inheritance", 0),
    n("cycling", "Cycling", 1),
    n("none", "None", 2),
];

/// Juggernaut movement and ball speed (settings 96 and 45).
const SPEEDS: &[Named] = &[
    n("slow", "Slow", 0),
    n("normal", "Normal", 1),
    n("fast", "Fast", 2),
];

// ---- the table ----

/// Engine indices of the carriers with game-type settings.
const CTF: u8 = 1;
const ODDBALL: u8 = 3;
const KING: u8 = 4;
const JUGGERNAUT: u8 = 7;
const TERRITORIES: u8 = 8;
const ASSAULT: u8 = 9;

/// The most an `i32` count or time in the record keeps.
const MAX: i32 = 0x7fff;

const fn row(
    name: &'static str,
    label: &'static str,
    kind: Kind,
    store: Store,
    carriers: &'static [u8],
) -> Row {
    Row {
        name,
        label,
        kind,
        store,
        carriers,
        zero_is_none: false,
    }
}

const fn none_at_zero(r: Row) -> Row {
    Row {
        zero_is_none: true,
        ..r
    }
}

const fn int(lo: i32, hi: i32) -> Kind {
    Kind::Int { lo, hi }
}

const fn secs(lo: i32, hi: i32) -> Kind {
    Kind::Seconds { lo, hi }
}

const fn choice(hi: i32, names: &'static [Named]) -> Kind {
    Kind::Choice { lo: 0, hi, names }
}

const fn percent() -> Kind {
    Kind::Percent { lo: 10, hi: 400 }
}

const fn setting(id: u32) -> Store {
    Store::Setting(id)
}

const ALL: &[u8] = &[];

/// Every setting a script may name, by index. Append only.
pub const TABLE: &[Row] = &[
    // The names `variant.<name>` reads.
    row("team_play", "Team Play", Kind::Bool, setting(91), ALL),
    none_at_zero(row(
        "score_limit",
        "Score to Win",
        int(0, MAX),
        Store::Score,
        ALL,
    )),
    none_at_zero(row(
        "time_limit",
        "Time Limit",
        secs(0, MAX),
        setting(10),
        ALL,
    )),
    row("lives", "Lives per Round", int(0, MAX), setting(16), ALL),
    row(
        "respawn_time",
        "Respawn Time",
        secs(0, MAX),
        setting(17),
        ALL,
    ),
    row(
        "friendly_fire",
        "Friendly Fire",
        Kind::Bool,
        setting(24),
        ALL,
    ),
    row("rounds", "Rounds", int(0, 6), setting(0), ALL),
    // Every game type's.
    row(
        "suicide_penalty",
        "Suicide Penalty",
        secs(0, MAX),
        setting(18),
        ALL,
    ),
    row("shields", "Shields", choice(2, SHIELDS), setting(19), ALL),
    row(
        "motion_sensor",
        "Motion Sensor",
        Kind::Bool,
        setting(20),
        ALL,
    ),
    row("active_camo", "Active Camo", Kind::Bool, setting(21), ALL),
    row(
        "extra_damage",
        "Extra Damage",
        Kind::Bool,
        setting(104),
        ALL,
    ),
    row(
        "damage_resistance",
        "Damage Resistance",
        Kind::Bool,
        setting(105),
        ALL,
    ),
    row("max_players", "Max Players", int(0, 16), setting(90), ALL),
    row("team_count", "Team Count", int(0, 8), setting(93), ALL),
    row(
        "starting_weapon",
        "Starting Weapon",
        choice(20, WEAPONS),
        setting(89),
        ALL,
    ),
    row(
        "secondary_weapon",
        "Secondary Weapon",
        choice(20, WEAPONS),
        setting(94),
        ALL,
    ),
    row(
        "starting_grenades",
        "Starting Grenades",
        Kind::Bool,
        setting(88),
        ALL,
    ),
    row(
        "weapons_on_map",
        "Weapons on Map",
        choice(19, WEAPON_SETS),
        setting(83),
        ALL,
    ),
    row(
        "grenades_on_map",
        "Grenades on Map",
        Kind::Bool,
        setting(86),
        ALL,
    ),
    row(
        "weapon_respawn",
        "Weapon Respawn",
        choice(3, WEAPON_RESPAWN),
        setting(87),
        ALL,
    ),
    row(
        "overshield_pickups",
        "Overshield Pickups",
        Kind::Bool,
        setting(84),
        ALL,
    ),
    row(
        "camo_pickups",
        "Active Camo Pickups",
        Kind::Bool,
        setting(85),
        ALL,
    ),
    row(
        "light_vehicle",
        "Light Vehicle",
        choice(7, LIGHT_VEHICLES),
        setting(76),
        ALL,
    ),
    row(
        "second_light_vehicle",
        "Second Light Vehicle",
        choice(7, LIGHT_VEHICLES),
        setting(77),
        ALL,
    ),
    row(
        "heavy_vehicle",
        "Heavy Vehicle",
        choice(4, HEAVY_VEHICLES),
        setting(78),
        ALL,
    ),
    row(
        "flying_vehicle",
        "Flying Vehicle",
        choice(4, FLYING_VEHICLES),
        setting(79),
        ALL,
    ),
    row(
        "primary_turret",
        "Primary Turret",
        choice(6, TURRETS),
        setting(81),
        ALL,
    ),
    row(
        "secondary_turret",
        "Secondary Turret",
        choice(6, TURRETS),
        setting(82),
        ALL,
    ),
    // One game type's: the type is in the name.
    row(
        "juggernaut_overshield",
        "Juggernaut Overshield",
        Kind::Bool,
        setting(71),
        &[JUGGERNAUT],
    ),
    row(
        "juggernaut_active_camo",
        "Juggernaut Active Camo",
        Kind::Bool,
        setting(72),
        &[JUGGERNAUT],
    ),
    row(
        "juggernaut_motion_sensor",
        "Juggernaut Motion Sensor",
        Kind::Bool,
        setting(73),
        &[JUGGERNAUT],
    ),
    row(
        "juggernaut_damage_resistance",
        "Juggernaut Damage Resistance",
        Kind::Bool,
        setting(108),
        &[JUGGERNAUT],
    ),
    row(
        "juggernaut_movement",
        "Juggernaut Movement",
        choice(2, SPEEDS),
        setting(96),
        &[JUGGERNAUT],
    ),
    row(
        "oddball_ball_count",
        "Ball Count",
        int(0, 3),
        setting(43),
        &[ODDBALL],
    ),
    row(
        "oddball_ball_speed",
        "Ball Carrier Speed",
        choice(2, SPEEDS),
        setting(45),
        &[ODDBALL],
    ),
    row(
        "oddball_ball_toughness",
        "Ball Carrier Toughness",
        Kind::Bool,
        setting(102),
        &[ODDBALL],
    ),
    none_at_zero(row(
        "king_moving_hill",
        "Moving Hill",
        secs(0, MAX),
        setting(40),
        &[KING],
    )),
    row(
        "ctf_flag_type",
        "Flag Type",
        int(0, 2),
        Store::Paired {
            ctf: 50,
            assault: 59,
        },
        &[CTF, ASSAULT],
    ),
    row(
        "ctf_reset_time",
        "Flag Reset Time",
        secs(0, MAX),
        Store::Paired {
            ctf: 54,
            assault: 61,
        },
        &[CTF, ASSAULT],
    ),
    row(
        "ctf_touch_return",
        "Flag Touch Return",
        Kind::Bool,
        Store::Paired {
            ctf: 53,
            assault: 62,
        },
        &[CTF, ASSAULT],
    ),
    row(
        "ctf_flag_at_home",
        "Flag at Home to Score",
        Kind::Bool,
        setting(52),
        &[CTF],
    ),
    row(
        "assault_arm_time",
        "Bomb Arming Time",
        secs(0, 15),
        setting(63),
        &[ASSAULT],
    ),
    row(
        "territories_count",
        "Territory Count",
        int(1, 8),
        setting(74),
        &[TERRITORIES],
    ),
    row(
        "territories_contest_time",
        "Contest Time",
        secs(1, MAX),
        setting(98),
        &[TERRITORIES],
    ),
    // The `hx` block's extras.
    row(
        "gravity",
        "Gravity",
        percent(),
        Store::Ext(ExtField::Gravity),
        ALL,
    ),
    row(
        "damage",
        "Damage",
        percent(),
        Store::Ext(ExtField::Damage),
        ALL,
    ),
    row(
        "shield",
        "Shield Strength",
        percent(),
        Store::Ext(ExtField::Shield),
        ALL,
    ),
    row(
        "health",
        "Health",
        percent(),
        Store::Ext(ExtField::Health),
        ALL,
    ),
    row(
        "unlimited_grenades",
        "Unlimited Grenades",
        Kind::Bool,
        Store::Ext(ExtField::UnlimitedGrenades),
        ALL,
    ),
    // The settings the hopper defaults writer (`0x00193010`) clears and the
    // built-in matchmaking variants change (docs/live/live-matchmaking.md,
    // "6.8 The default set"): flag bits 4 and 5 (settings 12 and 13, no label of the
    // game's), setting 14 (`+0x58`), the respawn modifier and the second
    // heavy vehicle.
    row("setting_twelve", "Setting 12", Kind::Bool, setting(12), ALL),
    row("setting_thirteen", "Setting 13", Kind::Bool, setting(13), ALL),
    row(
        "join_in_progress",
        "Join in Progress",
        int(0, 2),
        setting(14),
        ALL,
    ),
    row(
        "respawn_modifier",
        "Respawn Modifier",
        choice(2, RESPAWN_MODIFIERS),
        setting(25),
        ALL,
    ),
    row(
        "second_heavy_vehicle",
        "Second Heavy Vehicle",
        choice(4, HEAVY_VEHICLES),
        setting(80),
        ALL,
    ),
];

/// The row at `index`.
pub fn row_at(index: u16) -> Option<&'static Row> {
    TABLE.get(index as usize)
}

/// The row named `name` and its index.
pub fn named(name: &str) -> Option<(u16, &'static Row)> {
    TABLE
        .iter()
        .position(|r| r.name == name)
        .map(|i| (i as u16, &TABLE[i]))
}

/// Whether the row at `index` applies to the carrier with engine index
/// `carrier`.
pub fn applies(index: u16, carrier: u8) -> bool {
    row_at(index).is_some_and(|r| r.applies(carrier))
}

/// The row name nearest `name`, for "did you mean": at most two edits.
pub fn nearest(name: &str) -> Option<&'static str> {
    TABLE
        .iter()
        .map(|r| (distance(name, r.name), r.name))
        .filter(|(d, _)| *d <= 2 && *d < name.len().max(1))
        .min()
        .map(|(_, n)| n)
}

/// The text of a row's value: "On", "None", a choice's label, "5 minutes",
/// "50%", or the number.
pub fn value_label(index: u16, value: i32) -> String {
    let Some(r) = row_at(index) else {
        return value.to_string();
    };
    if r.zero_is_none && value == 0 {
        return "None".into();
    }
    match r.kind {
        Kind::Bool => (if value != 0 { "On" } else { "Off" }).to_string(),
        Kind::Int { .. } => value.to_string(),
        Kind::Seconds { .. } => duration(value),
        Kind::Choice { names, .. } => names
            .iter()
            .find(|c| c.value == value)
            .map_or_else(|| value.to_string(), |c| c.label.to_string()),
        Kind::Percent { .. } => format!("{value}%"),
    }
}

/// Seconds in words: "1 second", "5 minutes", "1 hour 30 minutes".
fn duration(seconds: i32) -> String {
    let unit = |n: i32, one: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {one}s")
        }
    };
    if seconds <= 0 {
        return unit(seconds, "second");
    }
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    let mut parts = Vec::new();
    if h > 0 {
        parts.push(unit(h, "hour"));
    }
    if m > 0 {
        parts.push(unit(m, "minute"));
    }
    if s > 0 {
        parts.push(unit(s, "second"));
    }
    parts.join(" ")
}

/// Edit distance, for "did you mean".
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::VARIANT_FIELDS;

    #[test]
    fn names_are_unique_snake_case_and_keep_the_variant_field_names() {
        for (i, r) in TABLE.iter().enumerate() {
            assert!(
                r.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{}",
                r.name
            );
            assert!(!r.label.is_empty() && r.label.len() < 32, "{}", r.name);
            assert_eq!(
                named(r.name).map(|(at, _)| at as usize),
                Some(i),
                "{} twice",
                r.name
            );
            let (lo, hi) = r.kind.range();
            assert!(lo <= hi, "{}", r.name);
            for c in r.kind.names() {
                assert!(
                    r.in_range(c.value),
                    "{}: {} is out of range",
                    r.name,
                    c.name
                );
                assert_eq!(
                    r.choice(c.name),
                    Some(c.value),
                    "{}: {} twice",
                    r.name,
                    c.name
                );
            }
            for &c in r.carriers {
                assert!(
                    CARRIERS.iter().any(|&(_, e)| e == c),
                    "{}: carrier {c}",
                    r.name
                );
            }
        }
        // Every variant field is a setting of the same name.
        for (name, _) in VARIANT_FIELDS {
            assert!(named(name).is_some(), "variant.{name} has no row");
        }
    }

    #[test]
    fn score_limit_names_each_carriers_setting() {
        let (at, score) = named("score_limit").unwrap();
        for &(name, engine) in CARRIERS {
            let want = match name {
                "ctf" => 1,
                "slayer" => 2,
                "oddball" => 3,
                "king" => 4,
                "juggernaut" => 7,
                "territories" => 8,
                "assault" => 9,
                other => panic!("carrier {other}"),
            };
            assert_eq!(score.setting(engine), Some(want), "{name}");
            assert!(applies(at, engine), "{name}");
        }
        assert_eq!(score.setting(crate::SCRIPT_INDEX), Some(2));
        assert_eq!(score.setting(5), None, "an engine no script rides");
        assert_eq!(score.none_value(), Some(0));
    }

    #[test]
    fn a_game_type_setting_applies_to_its_carrier_only() {
        let (at, jugg) = named("juggernaut_movement").unwrap();
        assert!(applies(at, 7) && !applies(at, 2));
        assert_eq!(jugg.setting(7), Some(96));
        assert_eq!(jugg.setting(2), None);
        assert!(!jugg.applies(crate::SCRIPT_INDEX));
        assert_eq!(jugg.carrier_words(), "Juggernaut");
        let (_, flag) = named("ctf_flag_type").unwrap();
        assert_eq!(
            (flag.setting(1), flag.setting(9), flag.setting(2)),
            (Some(50), Some(59), None)
        );
        assert_eq!(flag.carrier_words(), "Capture the Flag and Assault");
        let (_, gravity) = named("gravity").unwrap();
        assert!(gravity.applies(3) && !gravity.is_record());
        assert_eq!(gravity.setting(3), None, "an hx field is no record setting");
        assert!(!applies(TABLE.len() as u16, 2));
    }

    #[test]
    fn values_have_labels_and_none_where_the_row_says() {
        let at = |name| named(name).unwrap().0;
        assert_eq!(value_label(at("motion_sensor"), 0), "Off");
        assert_eq!(value_label(at("starting_weapon"), 9), "Shotgun");
        assert_eq!(value_label(at("starting_weapon"), 15), "15");
        assert_eq!(value_label(at("time_limit"), 0), "None");
        assert_eq!(value_label(at("time_limit"), 90), "1 minute 30 seconds");
        assert_eq!(value_label(at("respawn_time"), 1), "1 second");
        assert_eq!(value_label(at("gravity"), 50), "50%");
        assert_eq!(named("shields").unwrap().1.none_value(), Some(1));
        assert_eq!(named("light_vehicle").unwrap().1.none_value(), Some(7));
        assert_eq!(named("lives").unwrap().1.none_value(), None);
        assert_eq!(nearest("score_limt"), Some("score_limit"));
        assert_eq!(nearest("zzzz"), None);
    }
}
