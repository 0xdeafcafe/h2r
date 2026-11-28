//! Guest addresses and offsets from `docs/live/live-gamestate.md`.
//!
//! The structure offsets and global addresses are `guest_layout`'s
//! (docs/engine/guest-layout.md); the names here alias them for the
//! readers that add by hand. A layout fix goes there, once.

use guest_layout::view::objects::{Body, Header as ObjectHeader};
use guest_layout::view::players::Player;

pub const GAME_GLOBALS_PTR: u32 = guest_layout::points::GAME_GLOBALS.base;
pub const GAME_TIME_PTR: u32 = guest_layout::points::GAME_TIME.base;
pub const ENGINE_GLOBALS_PTR: u32 = guest_layout::global::GAME_ENGINE.base;
pub const PLAYERS_PTR: u32 = guest_layout::points::PLAYERS.base;
pub const OBJECTS_PTR: u32 = guest_layout::points::OBJECTS.base;
pub const CACHE_MAP_NAME: u32 = guest_layout::global::CACHE_MAP_NAME.base;

// The hooked functions (match start, tick, end, kill, weapon fire) are
// named in `guest_layout::points`.

pub const OPTIONS: u32 = guest_layout::game::OPTIONS;
pub const OPTIONS_TYPE: u32 = guest_layout::GameOptions::MODE.offset;
pub const OPTIONS_TICK_RATE: u32 = guest_layout::GameOptions::TICK_RATE.offset;
/// Campaign difficulty, `i16`: 0 easy, 1 normal, 2 heroic, 3 legendary.
/// Confirmed: `0x00138110` defaults it to 1 and the menu's campaign start
/// (`0x0023643f`) sets it from the selection; level boot writes it the same
/// way (docs/runtime/level-boot.md).
pub const OPTIONS_DIFFICULTY: u32 = guest_layout::GameOptions::DIFFICULTY.offset;
/// The skull array (docs/game/skulls.md): one byte per skull, 1 = on. Written
/// only by the pickup (`0x1df3e0`) and our campaign start; every reader
/// gates on a campaign (confirmed).
pub const SKULLS: u32 = guest_layout::global::SKULLS.base;
pub const SKULL_COUNT: usize = 15;

/// The campaign difficulty's name, `None` outside 0..=3.
pub fn difficulty_name(value: i16) -> Option<&'static str> {
    usize::try_from(value)
        .ok()
        .and_then(|i| live_proto::game::DIFFICULTIES.get(i).copied())
}
pub const VARIANT: u32 = guest_layout::game::VARIANT;
pub const VARIANT_ENGINE: u32 = guest_layout::GameVariant::ENGINE.offset;
pub const VARIANT_FLAGS: u32 = guest_layout::GameVariant::FLAGS.offset;
pub const VARIANT_SCORE_LIMIT: u32 = guest_layout::GameVariant::SCORE_LIMIT.offset;
pub const VARIANT_TIME_LIMIT: u32 = guest_layout::GameVariant::TIME_LIMIT.offset;
pub const GAME_ENDED: u32 = guest_layout::GameGlobals::GAME_ENDED.offset;
pub const TIME_TICK: u32 = guest_layout::GameTime::TICK.offset;

// Datum arrays, players and object headers are read through
// `guest_layout::view`; these names alias its offsets for the readers that still
// add them by hand and for the test fixtures.
pub const DATUM_HIGH_WATER: u32 = guest_layout::datum::layout::HIGH_WATER;
pub const DATUM_DATA: u32 = guest_layout::datum::layout::DATA;
#[allow(dead_code)]
pub const PLAYER_SIZE: u32 = Player::SIZE;
#[allow(dead_code)]
pub const PLAYER_SALT: u32 = Player::SALT;
#[allow(dead_code)]
pub const PLAYER_FLAGS: u32 = Player::FLAGS;
#[allow(dead_code)]
pub const PLAYER_LOCAL_USER: u32 = Player::LOCAL_USER;
/// The local controller index (`i32`, `-1` for a remote player;
/// docs/live/live-gamestate.md, Players).
pub const PLAYER_CONTROLLER: u32 = guest_layout::Player::CONTROLLER.offset;
#[allow(dead_code)]
pub const PLAYER_UNIT: u32 = Player::UNIT;
#[allow(dead_code)]
pub const PLAYER_NAME: u32 = guest_layout::view::players::NAME;
#[allow(dead_code)]
pub const PLAYER_TEAM: u32 = Player::TEAM;
#[allow(dead_code)]
pub const PLAYER_NAME_CODE_UNITS: usize = guest_layout::view::players::NAME_UNITS;

// Player appearance (profile colours and emblem layers). The player record's
// confirmed local user index (`PLAYER_LOCAL_USER`, 0..3) selects one of four
// local player slots; a slot signed in with a profile (flags bit 4) embeds
// the 0x1e0-byte profile raw at `PLAYER_SLOT_PROFILE`, and the profile's
// appearance block sits `PROFILE_APPEARANCE` into it. Every offset below is
// "likely" (same-hash community decompilation; docs/live/live-gamestate.md, local
// player slots), not runtime-validated, so the reader range-checks each
// value and drops what fails. Remote players' profiles never pass through
// the local slots: their appearance reads as empty until the in-record
// location is validated.
pub const PLAYER_SLOTS: u32 = guest_layout::global::PLAYER_SLOTS.base; // likely: 4 slots, one per controller
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

pub const ENGINE_STATBORG: u32 = guest_layout::game::STATBORG;
pub const ENGINE_PLAYER_STATS: u32 = guest_layout::game::PLAYER_STATS;
pub const ENGINE_TEAM_STATS: u32 = guest_layout::game::TEAM_STATS;
pub const PLAYER_STAT_SIZE: u32 = guest_layout::game::PLAYER_STATS_STRIDE;
pub const TEAM_STAT_SIZE: u32 = guest_layout::game::TEAM_STATS_STRIDE;
pub const STAT_SCORE: u32 = guest_layout::game::PlayerStats::SCORE.offset;
pub const STAT_ROUNDS_SCORE: u32 = guest_layout::game::PlayerStats::ROUNDS_SCORE.offset;
pub const ENGINE_GAME_END_SEEN: u32 = guest_layout::game::GAME_END_SEEN;
pub const STAT_KILLS: u32 = guest_layout::game::PlayerStats::KILLS.offset;
pub const STAT_DEATHS: u32 = guest_layout::game::PlayerStats::DEATHS.offset;
pub const STAT_SUICIDES: u32 = guest_layout::game::PlayerStats::SUICIDES.offset;
pub const STAT_ASSISTS: u32 = guest_layout::game::PlayerStats::ASSISTS.offset;

pub const OBJECT_HEADER_SIZE: u32 = ObjectHeader::SIZE;
#[allow(dead_code)]
pub const OBJECT_SALT: u32 = ObjectHeader::SALT;
pub const OBJECT_TYPE: u32 = ObjectHeader::KIND;
pub const OBJECT_BODY: u32 = ObjectHeader::BODY;
pub const BODY_PARENT: u32 = Body::PARENT;
pub const BODY_POSITION: u32 = Body::POSITION;
pub const BODY_FORWARD: u32 = Body::FORWARD;
pub const BODY_UP: u32 = Body::UP;
/// Damage flags (`u16`); bit 2 is body depleted (`0x000d6800`).
pub const BODY_DAMAGE_FLAGS: u32 = Body::DAMAGE_FLAGS;
pub const BODY_DEPLETED: u16 = guest_layout::object::DEPLETED;
pub const OBJECT_TYPE_BIPED: u8 = guest_layout::ObjectType::Biped as u8;
pub const OBJECT_TYPE_VEHICLE: u8 = guest_layout::ObjectType::Vehicle as u8;
/// Object body `+0`: the definition tag's full salted datum.
pub const BODY_DEFINITION: u32 = Body::DEFINITION;
pub const OBJECT_TYPE_WEAPON: u8 = guest_layout::ObjectType::Weapon as u8;

// Tag index (docs/engine/tag_get_checked.md). The cache loader `0x00122a52`
// publishes the index header and reads the scenario datum from `+0x0c`
// (`0x00122ad8`); the map file's own index carries the same datums, which is
// how a host-side name table is checked against the loaded map.
pub const TAG_INDEX_PTR: u32 = guest_layout::points::TAG_INDEX.base;
pub const TAG_INDEX_SCENARIO: u32 = 0xc;

// Unit body, players frame fields (docs/live/live-gamestate.md, "Unit aim and held
// weapon"). Layout confirmed: `unit_new` (`0x000c4410`) sets the four weapon
// datums at `+0x218` and the two current-weapon indices at `+0x212`/`+0x213`
// to none, and initialises five vectors from the object forward vector
// (`+0x150`, `+0x15c`, `+0x168`, `+0x180`, `+0x18c`). Readers: `0x000c85c0`
// and `0x000cf3d0` index `+0x218` by the `i8` at `+0x212`.
/// The unit's actor datum, or none: what makes it AI-controlled. Confirmed:
/// `unit_new` sets it to none; `0x000c6810` indexes the actor datum array
/// (`[0x004f55f0]`, stride `0x888`) with it and copies the actor's team
/// (`actor+0x24`) into the unit.
pub const UNIT_ACTOR: u32 = guest_layout::Unit::ACTOR.offset;
/// The unit's player datum, or none (docs/live/live-gamestate.md, confirmed).
pub const UNIT_PLAYER: u32 = guest_layout::Unit::PLAYER.offset;
/// The unit's team, `i16`. Confirmed: `unit_new` takes it from the placement
/// (and in campaign, when that is 0 or -1, from the unit definition `+0xc0`);
/// `0x0014bfc0` copies the player's team, `0x000c6810` the actor's, and
/// `0x000b7930` hands it on to objects the unit spawns.
pub const UNIT_TEAM: u32 = guest_layout::Unit::TEAM.offset;
pub const UNIT_WEAPON_INDEX: u32 = guest_layout::Unit::WEAPON_INDEX.offset; // i8; -1 none
pub const UNIT_WEAPONS: u32 = guest_layout::Unit::WEAPONS.offset; // u32[4] weapon object datums
pub const UNIT_WEAPON_SLOTS: i8 = guest_layout::unit::WEAPON_SLOTS as i8;
/// likely: the aiming vector. `0x000cf3d0` casts the held weapon's
/// targeting ray from the unit origin along it (scaled by the weapon's
/// range), so it is world space; whether it is the smoothed vector or the
/// desired one (`+0x15c`, the one networked by `0x0009d450`) is not
/// established. Readers drop it unless finite and near unit length.
pub const UNIT_AIM: u32 = guest_layout::Unit::AIM.offset;

/// Weapon barrel fire, `0x00104150`: stdcall (weapon object datum, `i16`
/// barrel index), `ret 8`. Its one caller, `0x000fd980`, runs it for each
/// barrel whose pending-fire flag (bit 1 of the barrel state byte, weapon
/// body `+0x1a8 + 0x34 * barrel`) is set and then clears the flag. One call
/// is one shot (likely: a shotgun's pellets are one call; a battle rifle's
/// burst is three).
pub const WEAPON_FIRE: u32 = guest_layout::points::WEAPON_FIRE.base;

/// Fields whose names or semantics are only "likely" in the research document.
/// They are not needed for the initial event contract. Runtime validation can
/// update this one table before enabling any of them.
pub const LIKELY_FIELDS: &[(&str, u32)] = &[
    ("game_engine.round_start_tick", guest_layout::GameEngine::ROUND_START_TICK.offset),
    ("statborg.ninth_networked_stat", 0x10),
    ("statborg.multiple_kill_counter", 0x14),
    ("object_header.flags", 0x2),
    ("object_header.cluster_index", 0x4),
    ("damage_report.body_amount", 0x44),
    ("damage_report.shield_amount", 0x48),
    ("player_slot.signed_in_flag", 0x10),
    ("player_slot.profile", 0x18),
    ("profile.appearance", 0x118),
    ("profile.appearance.colors", 0x118),
    ("profile.appearance.emblem_foreground", 0x11d),
    ("profile.appearance.emblem_background", 0x11e),
    ("unit.aiming_vector", UNIT_AIM),
    ("campaign_team.names", 0),
    ("weapon_fire.one_call_per_shot", WEAPON_FIRE),
    ("biped.physics_mode", BIPED_PHYSICS_MODE),
    ("grenade.type_names", UNIT_GRENADE_INDEX),
    ("device.position", DEVICE_POSITION),
    ("damage_report.reporting_type_names", 0),
];

/// Pointer to a 0x24-byte game-state block that HaloScript's cinematics
/// drive (confirmed, docs/live/live-gamestate.md, "Cinematics"). Allocated
/// in the game state by `0x0013bf00` (size `0x24` at `0x0013bf27`, stored
/// at `0x0013bf4e`), so a checkpoint saves and restores it with the rest.
pub const CINEMATIC_GLOBALS_PTR: u32 = guest_layout::global::CINEMATIC_GLOBALS.base;
/// Its "a cinematic is playing" byte (confirmed): `cinematic_start`
/// (`0x002a9740` → `0x0013bff0`) sets `+4` and `+5` to 1 (`0x0013c01b`,
/// `0x0013c01e`); `cinematic_stop` (`0x002a9760` → `0x0013ca80`) clears
/// both (`0x0013caa9`, `0x0013cac2`). `0x0013cb40` returns `+5` (0 with no
/// block): the campaign pause menu is refused while it is set (docs/game/
/// settings-menu.md), and the tick budget `0x00146660`, the camera's
/// projection (`0x0002fc96`, the cinematic's aspect) and a dozen other
/// systems test it. Of the two, `+5` is the one the game itself asks.
pub const CINEMATIC_IN_PROGRESS: u32 = 0x5;

/// Active structure-BSP index in the game globals (`i16`, confirmed:
/// `0x0011c1b0` / `0x00137d40` index the scenario's BSP block with it).
pub const GLOBALS_ACTIVE_BSP: u32 = guest_layout::GameGlobals::ACTIVE_BSP.offset;

/// Translational velocity (`f32[3]`, confirmed, docs/live/live-gamestate.md),
/// in world units a **second**, not a tick. Confirmed on the projectile
/// update (`0x000f8ee2` on): it loads the seconds per tick (`T+0x04`) into
/// a local, copies the velocity (`lea esi,[ebp+0x88]`, `0x000f90a0`),
/// takes gravity times that tick length off its z (`0x000f9726`) and moves
/// the position (`+0x64`) by the velocity times the tick length
/// (`0x000f97df`..`0x000f9803`). The projectile's creation adds its tag's
/// initial velocity (`proj +0x17c`, world units a second, 400 for a bullet)
/// times a scale of 1.0 straight into it (`0x000f8487`..`0x000f84a0`). The
/// field is the object's, so units and vehicles hold the same unit. Frames
/// send it as `vel` unscaled; `motion` also uses its sign (a jump goes up),
/// and `speed` still comes from positions.
pub const BODY_VELOCITY: u32 = guest_layout::Object::VELOCITY.offset;
/// Body and shield vitality (`f32`, 1.0 full; an overshield charges the
/// shield past 1). Confirmed: `object_get_health` (`0x002a1a20`) returns
/// `+0xec` and `object_get_shield` (`0x002a1a90`) `+0xf0`, each 0 instead
/// while the damage flags' body-depleted bit ([`BODY_DEPLETED`]) is set
/// (`0x002a1a5e`, `0x002a1ace`); `boot::health` reads the same two.
pub const BODY_VITALITY: u32 = guest_layout::Object::HEALTH.offset;
pub const BODY_SHIELD_VITALITY: u32 = guest_layout::Object::SHIELD.offset;
/// The unit's seat in the vehicle it rides (`i16`, -1 none), the index in
/// the vehicle definition's seats block. Confirmed: `unit_new` (`0x000c44c1`)
/// sets it to -1; the enter-vehicle routine (`0x000ea1f0`, the unit in
/// `EAX`, the vehicle and seat on the stack, refused while the unit has a
/// parent) indexes the seats block with the seat (`imul edi,0xb0`,
/// `0x000ea330`) and stores it here (`0x000ea405`); leaving the vehicle
/// stores -1 (`0x000ea995`).
pub const UNIT_SEAT: u32 = guest_layout::Unit::SEAT.offset;
/// The scenario placement index (`i16`, -1 for objects the scenario did not
/// place): the index in the scenario block of the object's type.
/// Confirmed: `object_create` by name (`0x000bb670`) passes a name's type
/// (`+0x20`) and placement index (`+0x22`) to `0x000bf0f0` with the record
/// at the type's block (descriptor `+0xa`) plus index times stride
/// (`+0xe`); that hands the index to the placement builder `0x000d5060`,
/// which stores it in the placement data's `+0x10` (`0x000d520b`); and
/// `object_new` copies that to `+0x1a` (`0x000b7c87`). Objects not from a
/// placement get -1 (`0x000b7c63`; the placement data default `0x000b7a31`),
/// and an object a script recreates loses it (`0x000bb6cc`).
pub const BODY_PLACEMENT_INDEX: u32 = guest_layout::Object::PLACEMENT_INDEX.offset;
/// The netgame equipment entry that spawned the object (`i8`, -1 none).
/// Confirmed: the spawner `0x0015aad0` writes the entry index (its first
/// argument) after `object_new` (`0x0015ac15`); `object_new` sets -1
/// (`0x000b7db1`); deleting the object hands it back to its entry
/// (`0x000b85c3` calls `0x0015b220`, which clears it).
pub const BODY_NETGAME_EQUIPMENT: u32 = guest_layout::Object::NETGAME_EQUIPMENT.offset;
/// Object type 7 is a machine. Confirmed: the type table at `0x00468630`
/// is bipd, vehi, weap, eqip, garb, proj, scen, mach, ctrl, lifi, ssce,
/// bloc, crea (docs/formats/objects.md); the scarab of `03a`/`03b` is a `mach` tag.
pub const OBJECT_TYPE_MACHINE: u8 = guest_layout::ObjectType::Machine as u8;
/// Object type 3 is equipment and 5 a projectile (the same type table at
/// `0x00468630`). A projectile's position (`+0x64`) and velocity (`+0x88`)
/// are the ones the projectile update moves ([`BODY_VELOCITY`]); a stuck
/// one (a plasma grenade on a unit) has a parent and a parent-relative
/// transform, as every attached object.
pub const OBJECT_TYPE_EQUIPMENT: u8 = guest_layout::ObjectType::Equipment as u8;
pub const OBJECT_TYPE_PROJECTILE: u8 = guest_layout::ObjectType::Projectile as u8;

/// The loaded scenario tag (`scnr` root), confirmed (cache loader
/// publication; `0x000bb702` and `0x0015aad6` read placement blocks
/// through it).
pub const SCENARIO_PTR: u32 = guest_layout::global::SCENARIO.base;
/// Scenario placement blocks whose objects frames track: the object type
/// (header `+3`), the block's `(count, address)` offset in `scnr` and the
/// record stride, from the type table's descriptors (`+0xa`, `+0xe`;
/// docs/formats/objects.md, confirmed). Machines travel in `machines` and sound
/// scenery is not drawn, so neither is here.
pub const PLACEMENT_BLOCKS: &[(u8, u32, u32)] = &[
    (0, 0x60, 0x54),   // bipd
    (1, 0x70, 0x54),   // vehi
    (2, 0x90, 0x54),   // weap
    (3, 0x80, 0x38),   // eqip
    (6, 0x50, 0x5c),   // scen
    (8, 0xb8, 0x44),   // ctrl
    (9, 0xc8, 0x54),   // lifi
    (11, 0x328, 0x4c), // bloc
    (12, 0x368, 0x34), // crea
];
/// A placement record's position (`f32[3]`) and yaw, pitch, roll in
/// radians (`f32[3]`), as the placement builder reads them (`0x000d5140`,
/// `0x000d516e` into `0x0011df60`).
pub const PLACEMENT_POSITION: u32 = 0x8;
pub const PLACEMENT_ROTATION: u32 = 0x14;
/// Netgame equipment (`scnr+0x120`, stride `0x90`): the spawner reads the
/// entry at `index * 0x90` (`0x0015aaf3`), copies its position from `+0x40`
/// and converts its orientation at `+0x4c` with `0x0011df60`.
pub const NETGAME_EQUIPMENT: u32 = 0x120;
pub const NETGAME_EQUIPMENT_STRIDE: u32 = 0x90;
pub const NETGAME_EQUIPMENT_POSITION: u32 = 0x40;
pub const NETGAME_EQUIPMENT_ROTATION: u32 = 0x4c;
/// The most entries a placement block is read for (they count with `i16`).
pub const MAX_PLACEMENTS: u32 = 0x8000;
/// The device's normalized position. The updater at `0x0011546d` adds
/// the tick's travel to it. `0x00106930` passes it to the position
/// animation channel's normalized-frame setter (`0x001c6620`).
pub const DEVICE_POSITION: u32 = guest_layout::Device::POSITION.offset;
/// This device's position animation channel: `0x00106930` uses it as
/// the channel, and `0x001086e0` starts the position action here.
pub const DEVICE_POSITION_CHANNEL: u32 = guest_layout::unit::DEVICE_POSITION_CHANNEL;

/// Crouch amount, `f32` 0 (standing) to 1 (crouched); confirmed: the crouch
/// update at `0x000dd83f` moves it toward 1 while unit flags (`+0x134`) bit
/// 23 is set and toward 0 otherwise, by a rate times the tick length.
pub const UNIT_CROUCH: u32 = guest_layout::Unit::CROUCH.offset;
/// The selected grenade type (`i8`, -1 none); confirmed: `0x000cdff0` and
/// the HUD at `0x002759cf` index `UNIT_GRENADE_COUNTS` with it. likely: 0
/// frag, 1 plasma.
pub const UNIT_GRENADE_INDEX: u32 = guest_layout::Unit::GRENADE_INDEX.offset;
/// Grenades carried by type, `i8[2]`; confirmed as above.
pub const UNIT_GRENADE_COUNTS: u32 = guest_layout::Unit::GRENADE_COUNTS.offset;
/// likely: the biped's physics mode byte: 1 ground, 2 flying (airborne), 3
/// dead, 4 posture, 5 climbing, 6 melee. `biped_new` (`0x000de5f6`) clears
/// it; the crouch update tests it against 6 (`0x000dd897`) and other biped
/// code against 1, 2, 3 and 5. Names from Project Cartographer's
/// `e_biped_physics_mode`.
pub const BIPED_PHYSICS_MODE: u32 = guest_layout::Biped::PHYSICS_MODE.offset;

// The animation state (docs/live/replay-fidelity.md, "Animation state").
/// Where the object's animation manager is, `i16`: an offset from the
/// object's own data, -1 none. Confirmed: `object_new` (`0x000b7b40`)
/// builds the manager there (`0x000b80e0` into the constructor
/// `0x001cafc0`) and gives it the model's graph (`0x001cb0d0`); the custom
/// animation start (`0x001101e0`) checks it against -1 and works on
/// `object + offset`. Read by 113 sites.
pub const OBJECT_ANIMATION_MANAGER: u32 = guest_layout::Object::ANIMATION_MANAGER.offset;
/// The manager's animation graph, a `jmad` tag datum (-1 none). Confirmed:
/// `0x001ccae0` and `0x001cb410` refuse a manager whose `+0x68` is -1 and
/// pass it on as the channel's graph.
pub const MANAGER_GRAPH: u32 = 0x68;
/// The manager's first channel (of three, `0x20` bytes each, built by
/// `0x001cafc0`): the animation it plays; the other two blend out the ones
/// before. Confirmed: `0x001ccae0` starts an animation on the manager's own
/// address as the channel (`0x001cb410` into the channel set `0x001c6470`).
pub const MANAGER_CHANNEL: u32 = 0x0;
/// A channel's graph datum (`u32`, -1 none); confirmed, `0x001c6470`
/// writes it, `0x001c6c80` checks it.
pub const CHANNEL_GRAPH: u32 = 0x0;
/// A channel's animation: `i16` graph (-1 the channel's graph, else a
/// position in its inheritance list) then `i16` animation index (-1 none).
/// Confirmed: `0x001c6470` refuses a reference whose high half is -1 and
/// stores it here; `0x001c6560` resolves it through `0x001daea0`.
pub const CHANNEL_ANIMATION: u32 = 0x4;
/// A channel's frame, `f32`, 0 to the animation's frame count minus one.
/// Confirmed: the frame setter `0x001c6560` clamps it to that and stores it
/// here, then advances it by the tick length times the rate at `+0x18`.
pub const CHANNEL_FRAME: u32 = 0x1c;
/// Offsets a manager can be at: past the object's fixed fields, inside its
/// data.
pub const MANAGER_OFFSETS: std::ops::Range<i16> = 0x12c..0x2000;
/// Unit flags (`u32`). Bit 23 is the crouch request: the crouch update at
/// `0x000dd83f` tests it, and the biped physics update sets it at
/// `0x000e291b` while crouch is held and clears it at `0x000e2928` only
/// when the character physics reports room to stand (`0x001e59d8`), which
/// is why a biped under a low ceiling stays crouched with the button up.
pub const UNIT_FLAGS: u32 = guest_layout::Unit::FLAGS.offset;
pub const UNIT_FLAG_CROUCH: u32 = guest_layout::unit::CROUCH_REQUEST;
/// Biped flags (`u16`). Bit 0 counts airborne ticks into
/// `BIPED_AIRBORNE_TICKS` (`0x000dd608`); `0x000e4050` (the airborne test
/// the crouch-to-velocity terms use) wants it set. likely: bit 0 airborne.
pub const BIPED_FLAGS: u32 = guest_layout::Biped::FLAGS.offset;
/// Ticks bit 0 of `BIPED_FLAGS` has been set (`i8`, capped at 127;
/// `0x000dd611`).
pub const BIPED_AIRBORNE_TICKS: u32 = guest_layout::Biped::AIRBORNE_TICKS.offset;
/// Hitbox offset counter (`i8`). A melee lunge started standing sets it
/// to minus (standing − crouching height) / (tick length ×
/// `HITBOX_OFFSET_RATE`) (`0x000e29b4`) and shrinks the capsule to
/// crouch height; it steps one tick toward 0 each tick outside the lunge
/// (`0x000e2890`). While it is not 0 the Havok body is placed
/// at the object's origin minus `counter × tick length ×
/// HITBOX_OFFSET_RATE` in z (`0x000e0d70`, used at `0x001d405b`).
pub const BIPED_HITBOX_OFFSET: u32 = guest_layout::Biped::HITBOX_OFFSET.offset;
/// The ground support the biped physics update (`0x000dd990`) keeps from
/// the character's contacts. Each update clears bit 4 (`0x10`) of the byte
/// at `+0x349`, the high byte of `BIPED_FLAGS` (so their bit 12),
/// (`0x000ddb91`) and sets it again when it copies a support
/// contact (`0x000ddeda`); the five words at `+0x354..+0x364` name what
/// supports the biped (a structure surface: `+0x35c` the key's low 16
/// bits, `+0x354` its 13-bit field, `+0x360` -1, `0x000ddcd0..0x000ddce5`;
/// -1 throughout when nothing does, `0x000ddbb4..0x000ddbcc`). The support
/// contact's point (contact `+0x1c`) goes to `+0x380` and `+0x368`, its
/// normal (contact `+0x28`) to `+0x374` (`0x000dde99..0x000ddef5`).
pub const BIPED_SUPPORT_FLAGS: u32 = guest_layout::unit::SUPPORT_FLAGS;
pub const BIPED_SUPPORT_COPIED: u8 = guest_layout::unit::SUPPORT_COPIED;
pub const BIPED_SUPPORT_WORDS: u32 = guest_layout::unit::SUPPORT_WORDS;
pub const BIPED_SUPPORT_POINT: u32 = guest_layout::Biped::SUPPORT_POINT.offset;
pub const BIPED_SUPPORT_NORMAL: u32 = guest_layout::Biped::SUPPORT_NORMAL.offset;
pub const BIPED_SUPPORT_CONTACT_POINT: u32 = guest_layout::Biped::SUPPORT_CONTACT_POINT.offset;
/// The ground mode's state: `0x001e5a60` hands the physics-mode block
/// (`BIPED_PHYSICS_MODE`) `+0x10` to the ground test `0x001f19c0`, which
/// first takes the length of its first three floats (`0x001f19ff..0x001f1a35`).
pub const BIPED_GROUND_STATE: u32 = guest_layout::Biped::GROUND_STATE.offset;
/// The character contacts the ground test walks, on the physics component
/// (`PHYSICS_COMPONENTS_PTR`, indexed by `BODY_PHYSICS` as in
/// `0x001e68a0`'s `+0x40`): pointer `+0x88`, count `+0x8c`, `0x48` each
/// (`0x001f1aa0`, `0x001f1aca`, `0x001f1be5`). In an element: `+0x04` the
/// shape key the support is named by (`0x000ddc5e`, type `key >> 29`),
/// `+0x14` a second id the test compares with -1 (`0x001f1b75`) and with
/// the biped's `+0x408` (`0x001f2395`), `+0x1c` the point (`0x000dde99`,
/// `0x001f2e8a`), `+0x28` the normal (`0x001f1ae0..0x001f1afc`), `+0x38` a
/// float whose magnitude must reach 1e-4 for the first pass to take the
/// contact (`0x001f1bb8`), `+0x44` flags (bit 1 and bit 2 read at
/// `0x001f1b33` and `0x001f1b4c`).
pub const COMPONENT_CHARACTER_CONTACTS: u32 = 0x88;
pub const COMPONENT_CHARACTER_CONTACT_COUNT: u32 = 0x8c;
pub const CHARACTER_CONTACT_SIZE: u32 = 0x48;
pub const CHARACTER_CONTACT_KEY: u32 = 0x04;
pub const CHARACTER_CONTACT_OTHER: u32 = 0x14;
pub const CHARACTER_CONTACT_POINT: u32 = 0x1c;
pub const CHARACTER_CONTACT_NORMAL: u32 = 0x28;
pub const CHARACTER_CONTACT_VALUE: u32 = 0x38;
pub const CHARACTER_CONTACT_FLAGS: u32 = 0x44;
/// At most this many character contacts are read.
pub const CHARACTER_CONTACTS_MAX: u32 = 32;
/// The Havok body behind an object (confirmed, `0x001d08e0`): the
/// object's physics component index at body `+0xb4` (-1 none) indexes the
/// component array (`PHYSICS_COMPONENTS_PTR`, element `0xa0`); a component
/// keeps its rigid bodies at `+0x70` (entry `0x60`, the rigid body pointer
/// at `+0x40`); a rigid body's motion is at `+0x3c`, its linear velocity at
/// motion `+0x40` (`0x000dc67d`) and its world transform at motion `+0x80`
/// with the position at `+0xb0`.
pub const BODY_PHYSICS: u32 = guest_layout::Object::PHYSICS.offset;
pub const PHYSICS_COMPONENTS_PTR: u32 = guest_layout::global::PHYSICS_COMPONENTS.base;
pub const PHYSICS_COMPONENT_SIZE: u32 = 0xa0;
pub const COMPONENT_BODIES: u32 = 0x70;
pub const COMPONENT_BODY_RIGID: u32 = 0x40;
pub const RIGID_MOTION: u32 = 0x3c;
pub const MOTION_VELOCITY: u32 = 0x40;
pub const MOTION_POSITION: u32 = 0xb0;
// The Havok world's offsets live in `sim_havok::layout`; re-exported here.
pub use sim_havok::layout::*;
/// The world pointer (`0x001c2f07` stores it after `0x003101c0`).
pub const HAVOK_WORLD_PTR: u32 = guest_layout::global::HAVOK_WORLD.base;
/// An object header's body size (`u16`): the allocator `0x000bc2c6` stores
/// the type descriptor's runtime body size (`+8`, `0x000b7bd6`) here.
pub const OBJECT_BODY_SIZE: u32 = guest_layout::ObjectHeader::BODY_SIZE.offset;
/// Units per second the hitbox offset decays by (`0x004407b8`).
pub const HITBOX_OFFSET_RATE: f32 = 0.15;
pub const BIPED_MODE_GROUND: u8 = guest_layout::unit::MODE_GROUND;
pub const BIPED_MODE_FLYING: u8 = guest_layout::unit::MODE_FLYING;
pub const BIPED_MODE_MELEE: u8 = guest_layout::unit::MODE_MELEE;
/// The first magazine's reserve and loaded rounds (`i16`), weapon body;
/// confirmed (docs/live/live-gamestate.md, `0x00101a10`).
pub const WEAPON_MAGAZINE_RESERVE: u32 = guest_layout::Weapon::MAGAZINE_RESERVE.offset;
pub const WEAPON_MAGAZINE_LOADED: u32 = guest_layout::Weapon::MAGAZINE_LOADED.offset;

// The damage report (`guest_layout::points::DAMAGE_REPORT`, `0x000d9640`): custom
// ABI, `EDI` = the report, one stack argument (the victim object datum),
// `ret 4`. Called for every damage instance applied to an object (four
// callers: `0x0009c76e`, `0x000d8292`, `0x000d87a2`, `0x000dc211`); it adds
// the report's amounts to the victim's recent-damage accumulators (`+0xf8`,
// `+0x100`). The report's fields:
pub const REPORT_CODE: u32 = 0x0; // u8: low 6 bits damage reporting type
pub const REPORT_EFFECT: u32 = 0x8; // jpt! tag datum
pub const REPORT_OWNER_PLAYER: u32 = 0xc;
pub const REPORT_OWNER_OBJECT: u32 = 0x10;
pub const REPORT_BODY: u32 = 0x44; // likely: f32, fraction of body vitality
pub const REPORT_SHIELD: u32 = 0x48; // likely: f32, fraction of shield vitality

/// likely: the damage reporting type (the report code's low 6 bits) as a
/// hit `kind` and, for hand weapons, the weapon's tag stem. The order is the
/// multiplayer kill-feed list (Project Cartographer's
/// `e_damage_reporting_type`), not checked against the executable.
pub const fn damage_reporting_type(code: u8) -> (&'static str, Option<&'static str>) {
    match guest_layout::damage::reporting_type(code) {
        0 => ("guardians", None),
        1 => ("fall", None),
        2 => ("collision", None),
        3 => ("melee", None),
        4 => ("explosion", None),
        5 => ("bullet", Some("magnum")),
        6 => ("bullet", Some("plasma_pistol")),
        7 => ("bullet", Some("needler")),
        8 => ("bullet", Some("smg")),
        9 => ("bullet", Some("plasma_rifle")),
        10 => ("bullet", Some("battle_rifle")),
        11 => ("bullet", Some("covenant_carbine")),
        12 => ("bullet", Some("shotgun")),
        13 => ("bullet", Some("sniper_rifle")),
        14 => ("bullet", Some("beam_rifle")),
        15 => ("explosion", Some("rocket_launcher")),
        16 => ("explosion", Some("flak_cannon")),
        17 => ("explosion", Some("brute_shot")),
        18 => ("bullet", None),
        19 => ("bullet", Some("brute_plasma_rifle")),
        20 => ("melee", Some("energy_blade")),
        21 | 22 => ("grenade", None),
        23 => ("melee", Some("flag")),
        24 => ("melee", Some("assault_bomb")),
        25 => ("explosion", Some("assault_bomb")),
        26 => ("melee", Some("ball")),
        27 | 28 => ("turret", None),
        29..=38 => ("vehicle", None),
        39 | 40 => ("sentinel", None),
        41 => ("teleporter", None),
        _ => ("unknown", None),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KillSource {
    pub name: &'static str,
    pub headshot: Option<bool>,
}

/// Weapon or environmental cause and headshot from the depleting damage
/// report (or the fourth argument of a direct PLAYER_KILLED call).
/// The game groups a collision credited against another player, or damage
/// with modifier 3, as a splatter (docs/live/live-gamestate.md, section 7).
pub(crate) fn kill_source(code: u8, killed_other: bool) -> Option<KillSource> {
    use guest_layout::damage::{modifier, reporting_type};

    let (kind, weapon) = damage_reporting_type(code);
    if kind == "unknown" {
        return None;
    }
    let modifier = modifier(code);
    let source = match reporting_type(code) {
        _ if modifier == 3 => "splatter",
        2 if killed_other => "splatter",
        18 => "disintegrator",
        21 => "frag_grenade",
        22 => "plasma_grenade",
        27 => "human_turret",
        28 => "plasma_turret",
        29 => "banshee",
        30 => "ghost",
        31 => "mongoose",
        32 => "scorpion",
        33 => "spectre",
        34 => "spectre_turret",
        35 => "warthog",
        36 => "warthog_turret",
        37 => "wraith",
        38 => "tank",
        39 => "sentinel_beam",
        40 => "sentinel_rpg",
        _ => weapon.unwrap_or(kind),
    };
    let weapon = matches!(reporting_type(code), 5..=40) && modifier != 3;
    Some(KillSource { name: source, headshot: weapon.then_some(modifier == 1) })
}

/// likely (Project Cartographer's `e_unit_grenade_type`): grenade type
/// names by index.
pub const fn grenade_name(index: i8) -> Option<&'static str> {
    match index {
        0 => Some("frag"),
        1 => Some("plasma"),
        _ => None,
    }
}

/// likely: campaign team names by index, from the tag-definition enum
/// (`unit` default team, `+0xc0`); not checked against the executable. Other
/// indexes are sent as the number.
pub const fn campaign_team(index: i16) -> Option<&'static str> {
    match index {
        0 => Some("default"),
        1 => Some("player"),
        2 => Some("human"),
        3 => Some("covenant"),
        4 => Some("flood"),
        5 => Some("sentinel"),
        6 => Some("heretic"),
        7 => Some("prophet"),
        _ => None,
    }
}

/// Upper bound on object headers an actor scan reads (`objects_initialize`
/// creates 2,048).
pub const OBJECT_CAPACITY: u32 = 2048;

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

#[cfg(test)]
mod kill_source_tests {
    use super::{KillSource, kill_source};

    #[test]
    fn credited_sources_keep_grenades_melee_vehicles_and_the_world_distinct() {
        for (source, modifier, other, name, headshot) in [
            (10, 1, true, "battle_rifle", Some(true)),
            (10, 0, true, "battle_rifle", Some(false)),
            (3, 0, true, "melee", None),
            (3, 2, true, "melee", None), // assassination
            (20, 2, true, "energy_blade", Some(false)),
            (21, 0, false, "frag_grenade", Some(false)), // own grenade
            (22, 2, true, "plasma_grenade", Some(false)), // stick
            (27, 0, true, "human_turret", Some(false)),
            (36, 0, true, "warthog_turret", Some(false)),
            (30, 0, true, "ghost", Some(false)),
            (30, 3, true, "splatter", None),
            (2, 1, true, "splatter", None),
            (2, 0, false, "collision", None),
            (1, 1, false, "fall", None),
            (0, 0, false, "guardians", None),
            (4, 0, true, "explosion", None),
            (41, 0, false, "teleporter", None),
        ] {
            assert_eq!(
                kill_source(source | (modifier << 6), other),
                Some(KillSource { name, headshot }),
            );
        }
        assert_eq!(kill_source(0xff, true), None);
    }
}
