//! Guest addresses and offsets from `docs/live-gamestate.md`.
//!
//! Keep validation-sensitive layout changes here rather than in readers.

pub const GAME_GLOBALS_PTR: u32 = engine::points::GAME_GLOBALS.base;
pub const GAME_TIME_PTR: u32 = engine::points::GAME_TIME.base;
pub const ENGINE_GLOBALS_PTR: u32 = 0x004e_9ae8;
pub const PLAYERS_PTR: u32 = 0x004e_8c24;
pub const OBJECTS_PTR: u32 = 0x004e_0300;
pub const CACHE_MAP_NAME: u32 = 0x0054_7898;

// The hooked functions (match start, tick, end, kill, weapon fire) are
// named in `engine::points`.

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

// Player appearance (profile colours and emblem layers). The player record's
// confirmed local user index (`PLAYER_LOCAL_USER`, 0..3) selects one of four
// local player slots; a slot signed in with a profile (flags bit 4) embeds
// the 0x1e0-byte profile raw at `PLAYER_SLOT_PROFILE`, and the profile's
// appearance block sits `PROFILE_APPEARANCE` into it. Every offset below is
// "likely" (same-hash community decompilation; docs/live-gamestate.md, local
// player slots), not runtime-validated, so the reader range-checks each
// value and drops what fails. Remote players' profiles never pass through
// the local slots: their appearance reads as empty until the in-record
// location is validated.
pub const PLAYER_SLOTS: u32 = 0x0054_e8e0; // likely: 4 slots, one per controller
pub const PLAYER_SLOT_COUNT: u32 = 4;
pub const PLAYER_SLOT_SIZE: u32 = 0xc70; // likely
pub const PLAYER_SLOT_FLAGS: u32 = 0x0; // likely
pub const PLAYER_SLOT_SIGNED_IN: u32 = 0x10; // likely: flags bit 4
pub const PLAYER_SLOT_PROFILE: u32 = 0x18; // likely: 0x1e0 bytes, raw
pub const PROFILE_APPEARANCE: u32 = 0x118; // likely: 0x10 bytes
pub const APPEARANCE_PRIMARY: u32 = 0x0; // likely: colours[0], palette index
pub const APPEARANCE_SECONDARY: u32 = 0x1; // likely: colours[1]
pub const APPEARANCE_TERTIARY: u32 = 0x2; // likely: colours[2]
pub const APPEARANCE_QUATERNARY: u32 = 0x3; // likely: colours[3]
#[allow(dead_code)] // documents the block; not sent
pub const APPEARANCE_MODEL: u32 = 0x4; // likely; not sent
pub const APPEARANCE_EMBLEM_FOREGROUND: u32 = 0x5; // likely: 0..63
pub const APPEARANCE_EMBLEM_BACKGROUND: u32 = 0x6; // likely: 0..31
#[allow(dead_code)] // documents the block; not sent
pub const APPEARANCE_EMBLEM_FLAGS: u32 = 0x7; // likely; not sent

// Valid ranges for the appearance values (18 palette colours, 64 foreground
// layers, 32 background layers; the art manifest uses the same counts).
pub const PALETTE_COLOURS: u32 = 18;
pub const EMBLEM_FOREGROUNDS: u32 = 64;
pub const EMBLEM_BACKGROUNDS: u32 = 32;

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
/// Object body `+0`: the definition tag's full salted datum.
pub const BODY_DEFINITION: u32 = 0x0;
pub const OBJECT_TYPE_WEAPON: u8 = 2;

// Tag index (docs/engine/tag_get_checked.md). The cache loader `0x00122a52`
// publishes the index header and reads the scenario datum from `+0x0c`
// (`0x00122ad8`); the map file's own index carries the same datums, which is
// how a host-side name table is checked against the loaded map.
pub const TAG_INDEX_PTR: u32 = 0x0054_7f00;
pub const TAG_INDEX_SCENARIO: u32 = 0xc;

// Unit body, players frame fields (docs/live-gamestate.md, "Unit aim and held
// weapon"). Layout confirmed: `unit_new` (`0x000c4410`) sets the four weapon
// datums at `+0x218` and the two current-weapon indices at `+0x212`/`+0x213`
// to none, and initialises five vectors from the object forward vector
// (`+0x150`, `+0x15c`, `+0x168`, `+0x180`, `+0x18c`). Readers: `0x000c85c0`
// and `0x000cf3d0` index `+0x218` by the `i8` at `+0x212`.
pub const UNIT_WEAPON_INDEX: u32 = 0x212; // i8; -1 none
pub const UNIT_WEAPONS: u32 = 0x218; // u32[4] weapon object datums
pub const UNIT_WEAPON_SLOTS: i8 = 4;
/// likely: the aiming vector. `0x000cf3d0` casts the held weapon's
/// targeting ray from the unit origin along it (scaled by the weapon's
/// range), so it is world space; whether it is the smoothed vector or the
/// desired one (`+0x15c`, the one networked by `0x0009d450`) is not
/// established. Readers drop it unless finite and near unit length.
pub const UNIT_AIM: u32 = 0x168;

/// Weapon barrel fire, `0x00104150`: stdcall (weapon object datum, `i16`
/// barrel index), `ret 8`. Its one caller, `0x000fd980`, runs it for each
/// barrel whose pending-fire flag (bit 1 of the barrel state byte, weapon
/// body `+0x1a8 + 0x34 * barrel`) is set and then clears the flag. One call
/// is one shot (likely: a shotgun's pellets are one call; a battle rifle's
/// burst is three).
pub const WEAPON_FIRE: u32 = engine::points::WEAPON_FIRE.base;

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
    ("player_slot.signed_in_flag", 0x10),
    ("player_slot.profile", 0x18),
    ("profile.appearance", 0x118),
    ("profile.appearance.colors", 0x118),
    ("profile.appearance.emblem_foreground", 0x11d),
    ("profile.appearance.emblem_background", 0x11e),
    ("unit.aiming_vector", UNIT_AIM),
    ("weapon_fire.one_call_per_shot", WEAPON_FIRE),
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
