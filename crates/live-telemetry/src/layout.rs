//! Guest addresses and offsets from `docs/live-gamestate.md`.
//!
//! Keep validation-sensitive layout changes here rather than in readers.

pub const GAME_GLOBALS_PTR: u32 = 0x004e_6948;
pub const GAME_TIME_PTR: u32 = 0x0051_0c54;
pub const ENGINE_GLOBALS_PTR: u32 = 0x004e_9ae8;
pub const PLAYERS_PTR: u32 = 0x004e_8c24;
pub const OBJECTS_PTR: u32 = 0x004e_0300;
pub const CACHE_MAP_NAME: u32 = 0x0054_7898;

pub const MATCH_START: u32 = 0x0018_f000;
pub const GAME_TICK: u32 = 0x0013_7fe0;
pub const GAME_END: u32 = 0x0013_89c0;
pub const PLAYER_KILLED: u32 = 0x0015_ce70;

pub const OPTIONS: u32 = 0x8;
pub const OPTIONS_TYPE: u32 = 0x0;
pub const OPTIONS_TICK_RATE: u32 = 0x6;
pub const VARIANT: u32 = 0x134;
pub const VARIANT_ENGINE: u32 = 0x44;
pub const VARIANT_FLAGS: u32 = 0x48;
pub const VARIANT_SCORE_LIMIT: u32 = 0x50;
pub const VARIANT_TIME_LIMIT: u32 = 0x54;
pub const GAME_ENDED: u32 = 0x1128;
pub const TIME_TICK: u32 = 0x8;

pub const DATUM_HIGH_WATER: u32 = 0x38;
pub const DATUM_DATA: u32 = 0x44;
pub const PLAYER_SIZE: u32 = 0x21c;
pub const PLAYER_SALT: u32 = 0x0;
pub const PLAYER_FLAGS: u32 = 0x2;
pub const PLAYER_LOCAL_USER: u32 = 0x28;
pub const PLAYER_UNIT: u32 = 0x2c;
pub const PLAYER_NAME: u32 = 0x44;
pub const PLAYER_TEAM: u32 = 0xc0;
pub const PLAYER_NAME_CODE_UNITS: usize = 32;

pub const ENGINE_STATBORG: u32 = 0x304;
pub const ENGINE_PLAYER_STATS: u32 = 0x308;
pub const ENGINE_TEAM_STATS: u32 = 0x4c8;
pub const PLAYER_STAT_SIZE: u32 = 0x1c;
pub const TEAM_STAT_SIZE: u32 = 0x12;
pub const STAT_SCORE: u32 = 0x0;
pub const STAT_KILLS: u32 = 0x4;
pub const STAT_DEATHS: u32 = 0x6;
pub const STAT_SUICIDES: u32 = 0x8;
pub const STAT_ASSISTS: u32 = 0xc;

pub const OBJECT_HEADER_SIZE: u32 = 0xc;
pub const OBJECT_SALT: u32 = 0x0;
pub const OBJECT_TYPE: u32 = 0x3;
pub const OBJECT_BODY: u32 = 0x8;
pub const BODY_PARENT: u32 = 0x14;
pub const BODY_POSITION: u32 = 0x64;
pub const BODY_FORWARD: u32 = 0x70;

/// Fields whose names or semantics are only "likely" in the research document.
/// They are not needed for the initial event contract. Runtime validation can
/// update this one table before enabling any of them.
pub const LIKELY_FIELDS: &[(&str, u32)] = &[
    ("game_engine.round_start_tick", 0x70),
    ("statborg.ninth_networked_stat", 0x10),
    ("statborg.multiple_kill_counter", 0x14),
    ("object_header.flags", 0x2),
    ("object_header.cluster_index", 0x4),
    ("unit.vehicle_seat", 0x1fc),
    ("damage_report.body_amount", 0x44),
    ("damage_report.shield_amount", 0x48),
];

pub const fn variant_name(index: i32) -> Option<&'static str> {
    match index {
        1 => Some("CTF"),
        2 => Some("Slayer"),
        3 => Some("Oddball"),
        4 => Some("King of the Hill"),
        5 => Some("Race"),
        6 => Some("Headhunter"),
        7 => Some("Juggernaut"),
        8 => Some("Territories"),
        9 => Some("Assault"),
        _ => None,
    }
}
