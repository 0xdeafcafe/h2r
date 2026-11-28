//! Skulls of our own, hidden in the campaign (docs/game/skulls.md "Our skulls").
//!
//! Retail's skulls are fifteen bytes at `0x4f55dc` and its stock function
//! (`0x1df3e0`) takes only `n < 15`, so ours have no byte there: each is an
//! entry of `skulls::ABOUT` from index 15 (`skulls::COUNT`), kept per profile
//! like the others, and its effect is our own work on the feature plan:
//!
//! | n | Skull | How |
//! |---|---|---|
//! | 15 | Acrophobia | after each tick, an airborne local player's biped never sinks faster than its view asks: `vz = max(vz, aim_z * FLY_SPEED)` (look up to climb, level to hover, crouch to drop) |
//! | 16 | Low Gravity | the physics constants' gravity (`[0x51e9c4] + 0`) held at half of what the game set |
//! | 17 | Fall Damage | a local player's landing faster than [`FALL_SAFE`] drains shield, then health (`+0xf0`, `+0xec`); a drained body is killed the way `unit_kill` does |
//! | 18 | Unlimited Ammo | each held weapon's reserve (`W+0x22a`, per magazine) kept at the definition's maximum; the loaded rounds are left alone, so reloads still happen; energy weapons' charge kept full (`W+0x184 = 0`) |
//! | 19 | Unlimited Grenades | the unit's two grenade counts (`+0x23e`) kept at [`GRENADE_CAP`] |
//! | 20 | Invincible | the "cannot take damage" bit `object_cannot_take_damage` sets (`+0x10a` bit 7) kept on the local players' units |
//!
//! **Hidden collectables.** [`SPOTS`] places one per level. When a campaign
//! level is on a spot's BSP and at least one signed-in profile has not
//! collected its skull, the retail skull weapon
//! (`objects\weapons\multiplayer\ball\head_sp`, `head` on High Charity; its
//! tag datum read from the map file's tag index on the host) is created
//! there with the game's own `placement_data_new` (`0xb7930`) and
//! `object_new` (`0xb7b40`). When a local player holds it ([`held_by`]),
//! the skull is recorded for every signed-in profile (as retail does), turned
//! on for the rest of this level, its HUD message shown through the game's
//! (`0x24cbbf`, our text) and the object removed as `object_destroy` does
//! (`0xbeb30`, then `object_delete` `0xb8540`).
//!
//! **Calls into the game** happen at one point: a `before` on `game_tick`
//! that, when work is queued, answers with an async body which makes the
//! calls and then runs the tick itself (re-entering its chain, which this
//! time continues), as `coop_probe`'s lobby setup does.
//!
//! **Multiplayer.** Only Low Gravity is offered (`skulls_mp::MP`,
//! `live_proto::skulls::MULTIPLAYER`): gravity is one constant every
//! machine simulates with, so the host's set applied everywhere agrees. The
//! rest act on the local players only, which in a host-authoritative game
//! would leave the host and its guests disagreeing.
//!
//! `H2_SKULLS_REVEAL=1` logs each spot of the loaded level, its
//! coordinates and the distance from each local player every
//! [`REVEAL_EVERY`] ticks (stderr only; nothing in game).
//!
//! The logic is pure functions over plain data and [`GuestMemory`], tested
//! without a game.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use guest_layout::entry::{EntryFuture, GuestCall, Ret};
use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::view::{objects, players};
use settings::{Store, Value};

use crate::skulls::{self, ABOUT, COUNT, SKULLS};

/// How many skulls are ours.
pub const OURS: usize = 6;
pub const ACROPHOBIA: usize = COUNT;
pub const LOW_GRAVITY: usize = COUNT + 1;
pub const FALL_DAMAGE: usize = COUNT + 2;
pub const UNLIMITED_AMMO: usize = COUNT + 3;
pub const UNLIMITED_GRENADES: usize = COUNT + 4;
pub const INVINCIBLE: usize = COUNT + 5;

/// The feature id in the plan and `H2_FEATURES`.
pub const FEATURE: &str = "skulls-custom";

// ---- addresses (docs/game/skulls.md "Our skulls" has the evidence) ----

/// The game globals pointer: the options mode at `+8` (1 campaign, 2
/// multiplayer) and the active structure BSP (`i16`) at `+2`.
pub const GAME_GLOBALS: u32 = 0x004e_6948;
pub const GLOBALS_ACTIVE_BSP: u32 = 0x2;
pub const GLOBALS_MODE: u32 = 0x8;
/// The physics constants pointer (`[0x51e9c4]`): gravity is its first
/// float. `physics_set_gravity` (builtin 127, `0x2a2b30`) writes
/// `arg * [0x4455d8]` there; the constants' set-up (`0x1eb584`) and the
/// reset builtin (`0x2a2ad0`) write `[0x4455d8]` itself.
pub const PHYSICS_CONSTANTS: u32 = 0x0051_e9c4;
/// The standard gravity the game resets to (read at run time, never copied).
pub const STANDARD_GRAVITY: u32 = 0x0044_55d8;
/// `placement_data_new`: ESI the `0xc4`-byte placement, stdcall
/// `(definition datum, owner datum, 0)`, `ret 0xc`.
pub const PLACEMENT_NEW: u32 = 0x000b_7930;
pub const PLACEMENT_SIZE: u32 = 0xc4;
/// The placement's position (three floats; the netgame equipment spawn at
/// `0x15aba6` copies its entry's position here).
pub const PLACEMENT_POSITION: u32 = 0x1c;
/// `object_new`: stdcall `(placement)`, the new object's datum in EAX (-1 on
/// failure), `ret 4`.
pub const OBJECT_NEW: u32 = 0x000b_7b40;
/// `object_destroy`'s two steps (builtin 52, `0x2a14d0`): this answers
/// non-zero (AL) for an object it must not delete, stdcall `(datum)`, `ret 4`;
pub const OBJECT_DESTROY_REFUSES: u32 = 0x000b_eb30;
/// then `object_delete`, stdcall `(datum)`, `ret 4`.
pub const OBJECT_DELETE: u32 = 0x000b_8540;
/// The HUD message the skull pickup shows (`0x1df40d`): EDI the string id,
/// stdcall `(local user)`, `ret 4`.
pub const HUD_MESSAGE: u32 = 0x0024_cbbf;
/// What `unit_kill` (builtin 196, `0x2a3320`) calls after setting the kill
/// bit: EAX the object datum, plain `ret`.
pub const OBJECT_WAKE: u32 = 0x000b_7360;

/// Object body: health and shield, as `object_get_health` (`0x2a1a20`) and
/// `object_get_shield` (`0x2a1a90`) read them (fractions, 1.0 full).
pub const BODY_HEALTH: u32 = 0xec;
pub const BODY_SHIELD: u32 = 0xf0;
pub const BODY_VELOCITY: u32 = 0x88;
pub const BODY_PARENT: u32 = 0x14;
/// The damage flags word: `object_cannot_take_damage` (`0xd87e0`) sets bit
/// 7, `unit_kill` sets bit 5.
pub const BODY_DAMAGE_FLAGS: u32 = 0x10a;
pub const CANNOT_TAKE_DAMAGE: u16 = 0x80;
pub const KILL: u16 = 0x20;
/// Unit: aim vector (likely the world-space aim), held weapons, grenades.
pub const UNIT_AIM: u32 = 0x168;
pub const UNIT_WEAPONS: u32 = 0x218;
pub const UNIT_GRENADES: u32 = 0x23e;
/// Biped: crouch amount (0..1) and physics mode (likely: 1 ground, 2
/// airborne).
pub const BIPED_CROUCH: u32 = 0x2c4;
pub const BIPED_MODE: u32 = 0x3dc;
pub const MODE_GROUND: u8 = 1;
pub const MODE_AIRBORNE: u8 = 2;
/// Weapon: magazines (`0x10` each; reserve `+2`, loaded `+4`) and, for a
/// weapon with none, the charge used (`1 - charge`, as `0x101a10` writes).
pub const WEAPON_MAGAZINES: u32 = 0x228;
pub const MAGAZINE_STRIDE: u32 = 0x10;
pub const MAGAZINE_RESERVE: u32 = 0x2;
pub const WEAPON_CHARGE_USED: u32 = 0x184;
/// Weapon definition: the magazine block (count, pointer; `0x5c` each) and
/// each magazine's reserve maximum (`0x101a10` caps the reserve with it).
pub const WEAP_MAGAZINE_COUNT: u32 = 0x2c0;
pub const WEAP_MAGAZINE_BLOCK: u32 = 0x2c4;
pub const WEAP_MAGAZINE_STRIDE: u32 = 0x5c;
pub const WEAP_RESERVE_MAX: u32 = 0x0c;
/// At most two magazines are kept per weapon (`W+0x228` and `W+0x238`).
pub const MAX_MAGAZINES: u32 = 2;

// ---- tuning (ours) ----

/// Low Gravity's fraction of the game's gravity.
pub const GRAVITY_SCALE: f32 = 0.5;
/// Acrophobia: the climb or sink speed at full look up or down, world units
/// per second.
pub const FLY_SPEED: f32 = 4.0;
/// Crouching past this drops out of flight.
pub const CROUCHED: f32 = 0.5;
/// Fall Damage: landing speeds up to this (world units per second) are
/// safe; a full jump lands at about 2.5.
pub const FALL_SAFE: f32 = 3.0;
/// Vitality lost per world unit per second past [`FALL_SAFE`].
pub const FALL_DAMAGE_PER_SPEED: f32 = 0.3;
/// Unlimited Grenades: each type kept at this many.
pub const GRENADE_CAP: i8 = 4;
/// How often `H2_SKULLS_REVEAL` logs (ticks; 30 a second).
pub const REVEAL_EVERY: u32 = 150;

// ---- the hiding places ----

/// Where one of ours is hidden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    /// The skull (`skulls::SKULLS` index).
    pub skull: usize,
    /// The level's map (cache short name).
    pub map: &'static str,
    /// The structure BSP the spot is in: the skull is created only while
    /// it is the active one.
    pub bsp: i16,
    /// World position (a little above the floor; the weapon settles).
    pub at: [f32; 3],
    /// A vague hint.
    pub hint: &'static str,
}

/// One per level, from each level's real geometry: the render geometry
/// (`map.glb`, glTF (x, y, z) = game (x, z, -y)), the BSP each floor is in,
/// and the scenario's placements, read off the disc with this repo's
/// tools. Each was checked flat, with headroom, reachable on foot from the
/// placements and the start (a flood fill over the floors), and clear of
/// placed weapons. `at` is the floor plus 0.15.
pub static SPOTS: [Spot; OURS] = [
    // Uprising (BSP 1, deltacontrolroom_bsp1): a roofed platform about 2 above its
    // surroundings, stepped up to from the east, between Covenant supply
    // crates. Floor z 68.50, 5.6 headroom; 70 from the start, 134 from the
    // retail skull.
    Spot {
        skull: ACROPHOBIA,
        map: "08a_deltacliffs",
        bsp: 1,
        at: [134.40, 117.60, 68.65],
        hint: "among the supply crates on the raised platform past the first cliff descent",
    },
    // Delta Halo (BSP 0): a stone ledge at z 25 right behind the tall waterfall
    // (x -173..-165, falling z 50 to 0) south of the Forerunner bridge,
    // reached from the ruins path. Floor z 25.27, 7.8 headroom under the
    // overhang; 131 from the retail skull.
    Spot {
        skull: LOW_GRAVITY,
        map: "05a_deltaapproach",
        bsp: 0,
        at: [-168.00, -302.50, 25.42],
        hint: "behind the falling water above the gorge bridge",
    },
    // Metropolis (BSP 1, earthcity_4): the east end of the northern covered
    // bridge-tube off the walkway at x 432, where it stops 12 above the
    // ground (a sniper rifle and battle rifle near its start). Floor z
    // -5.60, 1.7 headroom; 247 from the retail skull.
    Spot {
        skull: FALL_DAMAGE,
        map: "03b_newmombasa",
        bsp: 1,
        at: [452.50, 71.80, -5.45],
        hint: "at the very end of the gantry, where the bridge stops short",
    },
    // Outskirts (BSP 1, earthcity_2): on the beach, under the deck at the
    // seaward end of the long pier at y -460, past its solid block, among
    // the pilings. Floor z -7.27, the deck 11.9 above; 214 from the start.
    Spot {
        skull: UNLIMITED_AMMO,
        map: "03a_oldmombasa",
        bsp: 1,
        at: [-452.00, -468.00, -7.12],
        hint: "in the shade beneath the far end of the pier",
    },
    // Sacred Icon (BSP 0, sentinelhq_1): the south-west corner bay of the raised
    // ring in the octagonal room the first corridor opens into (centre
    // -387, 145), 3 above its floor. Floor z 4.20; 133 from the start.
    Spot {
        skull: UNLIMITED_GRENADES,
        map: "06a_sentinelwalls",
        bsp: 0,
        at: [-390.80, 134.20, 4.35],
        hint: "up in the corner bay of the great octagonal chamber",
    },
    // The Great Journey (BSP 0, deltacontrolroom_bsp0): the tip of the narrow
    // dead-end gully under the overhanging rock north-west of the start
    // (175.6, 10.0). Floor z 12.40, 9.3 headroom; 15 from the start.
    Spot {
        skull: INVINCIBLE,
        map: "08b_deltacontrol",
        bsp: 0,
        at: [167.00, 22.80, 12.55],
        hint: "at the end of the gully under the overhang, just north of where you begin",
    },
];

/// The spot of `map`, if one of ours is hidden there.
pub fn spot_for(map: &str) -> Option<&'static Spot> {
    SPOTS.iter().find(|s| s.map == map)
}

/// The skull weapon tags, by name: every level with a retail skull has the
/// first; High Charity has the second instead.
pub const SKULL_TAGS: [&str; 2] = [
    "objects\\weapons\\multiplayer\\ball\\head_sp",
    "objects\\weapons\\multiplayer\\ball\\head",
];

/// The skull weapon's datum among a map's (datum, group, name) rows: the
/// first of [`SKULL_TAGS`] present as a `weap`.
pub fn skull_tag<'a>(rows: impl IntoIterator<Item = (u32, [u8; 4], &'a str)>) -> Option<u32> {
    let rows: Vec<_> = rows.into_iter().filter(|(_, g, _)| g == b"weap").collect();
    SKULL_TAGS.iter().find_map(|want| {
        rows.iter()
            .find(|(_, _, n)| n.eq_ignore_ascii_case(want))
            .map(|(d, _, _)| *d)
    })
}

/// Whether to create the skull now: a campaign, the spot's BSP active,
/// someone still to find it, its tag known and none of ours alive.
pub fn should_spawn(
    campaign: bool,
    spot: &Spot,
    bsp: Option<i16>,
    uncollected: bool,
    tag: Option<u32>,
    alive: bool,
) -> bool {
    campaign && bsp == Some(spot.bsp) && uncollected && tag.is_some() && !alive
}

/// Whether anyone of `profiles` has yet to collect `skull`.
pub fn uncollected(store: &Store, profiles: &[String], skull: usize) -> bool {
    profiles
        .iter()
        .any(|p| skulls::state(store, p, skull) == Some(skulls::State::Locked))
}

// ---- the players ----

/// A local player's unit, as read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalUnit {
    /// The player's controller (local user) index.
    pub user: i16,
    /// The unit's datum.
    pub unit: u32,
    /// Its body.
    pub body: u32,
    /// Its object type (0 biped).
    pub kind: u8,
}

/// The local players' units: listed players with a local user and a live
/// unit.
pub fn local_units(m: &dyn GuestMemory) -> Result<Vec<LocalUnit>, MemFault> {
    player_units(m, true)
}

/// Every listed player's live unit (`local_only`: those with a local user).
/// A remote player's `user` is negative.
pub fn player_units(m: &dyn GuestMemory, local_only: bool) -> Result<Vec<LocalUnit>, MemFault> {
    let Some(array) = players::array(m)? else {
        return Ok(Vec::new());
    };
    if m.u32(array.array.addr.wrapping_add(guest_layout::datum::layout::DATA))? == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (_, p) in array.live(m, 16)? {
        let user = p.local_user(m)?;
        if (local_only && user < 0) || !p.listed(m)? {
            continue;
        }
        let Some(unit) = p.unit(m)? else { continue };
        if let Some((kind, body)) = objects::get(m, unit)? {
            out.push(LocalUnit {
                user,
                unit: unit.raw(),
                body: body.addr,
                kind,
            });
        }
    }
    Ok(out)
}

/// The weapons a unit holds (live datums only).
pub fn weapons_of(m: &dyn GuestMemory, unit_body: u32) -> Result<Vec<u32>, MemFault> {
    let mut out = Vec::new();
    for i in 0..4 {
        let w = m.u32(unit_body.wrapping_add(UNIT_WEAPONS + 4 * i))?;
        if w != u32::MAX {
            out.push(w);
        }
    }
    Ok(out)
}

/// Who picked our skull up: the index (in `units`) of the first local unit
/// that holds `skull` in a weapon slot or is its parent. Pure: `held` is
/// each unit's weapons as read.
pub fn held_by(skull: u32, parent: Option<u32>, units: &[(u32, Vec<u32>)]) -> Option<usize> {
    units
        .iter()
        .position(|(unit, held)| held.contains(&skull) || parent == Some(*unit))
}

// ---- the effects ----

/// Gravity's bookkeeping: the last value we wrote, the game's own value we
/// scaled and the scale, so a script's `physics_set_gravity` is scaled too,
/// a new scale rescales the game's value, and scale 1 gives it back.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gravity {
    pub written: Option<f32>,
    pub game: Option<f32>,
}

/// The value to write to the gravity constant, if any, given the scale
/// wanted now (Low Gravity's [`GRAVITY_SCALE`] times the variant's, 1.0
/// for the game's own) and the value there now.
pub fn gravity_step(scale: f32, now: f32, g: &mut Gravity) -> Option<f32> {
    let ours = g.written == Some(now);
    if scale != 1.0 {
        // The game's value: ours to rescale, or one the game (a reset, a
        // new map, a script) just set.
        let game = match (ours, g.game) {
            (true, Some(game)) => game,
            _ => now,
        };
        let v = game * scale;
        if ours && v == now {
            return None;
        }
        *g = Gravity {
            written: Some(v),
            game: Some(game),
        };
        Some(v)
    } else {
        let back = if ours { g.game } else { None };
        *g = Gravity::default();
        back
    }
}

/// Acrophobia: the vertical speed for an airborne biped moving at `vz`
/// with its aim's z `aim_z`, unless crouching. `None` leaves it.
pub fn fly(vz: f32, aim_z: f32, crouch: f32) -> Option<f32> {
    if crouch >= CROUCHED || !vz.is_finite() || !aim_z.is_finite() {
        return None;
    }
    let target = aim_z.clamp(-1.0, 1.0) * FLY_SPEED;
    (vz < target).then_some(target)
}

/// Fall Damage: vitality lost for a landing at `impact` world units per
/// second (0 up to [`FALL_SAFE`]).
pub fn fall_damage(impact: f32) -> f32 {
    if !impact.is_finite() || impact <= FALL_SAFE {
        return 0.0;
    }
    (impact - FALL_SAFE) * FALL_DAMAGE_PER_SPEED
}

/// Takes `damage` from the shield first, then the health: the new
/// (shield, health) and whether the body is drained.
pub fn take(shield: f32, health: f32, damage: f32) -> (f32, f32, bool) {
    let from_shield = damage.min(shield.max(0.0));
    let rest = damage - from_shield;
    let health = health - rest;
    if health <= 0.0 {
        (shield - from_shield, 0.0, true)
    } else {
        (shield - from_shield, health, false)
    }
}

/// A biped's last tick: airborne and its vertical speed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fall {
    pub airborne: bool,
    pub vz: f32,
}

/// The landing speed when a biped that was airborne last tick is not now.
pub fn landed(last: Fall, airborne_now: bool) -> Option<f32> {
    (last.airborne && !airborne_now && last.vz < 0.0).then_some(-last.vz)
}

/// Unlimited Grenades: the counts to keep.
pub fn topped(counts: [i8; 2]) -> [i8; 2] {
    counts.map(|c| c.max(GRENADE_CAP))
}

/// Unlimited Grenades on one unit's body: its two counts [`topped`].
/// Whether anything changed.
pub fn top_grenades(m: &mut dyn GuestMemory, body: u32) -> Result<bool, MemFault> {
    let now = [
        m.u8(body.wrapping_add(UNIT_GRENADES))? as i8,
        m.u8(body.wrapping_add(UNIT_GRENADES + 1))? as i8,
    ];
    let want = topped(now);
    if want != now {
        m.set_u8(body.wrapping_add(UNIT_GRENADES), want[0] as u8)?;
        m.set_u8(body.wrapping_add(UNIT_GRENADES + 1), want[1] as u8)?;
    }
    Ok(want != now)
}

/// A variant's Unlimited Grenades (docs/game/game-options.md section 2):
/// every player's unit, not only the local ones, since the host's counts
/// are the game's and they replicate. How many units changed.
pub fn variant_grenades(m: &mut dyn GuestMemory) -> Result<usize, MemFault> {
    let mut n = 0;
    for u in player_units(m, false)? {
        n += usize::from(top_grenades(m, u.body)?);
    }
    Ok(n)
}

/// Unlimited Ammo for one weapon: each magazine's reserve to its
/// definition's maximum; for a weapon with none, its charge to full.
/// Returns whether anything changed.
pub fn refill(m: &mut dyn GuestMemory, weapon_body: u32, def: u32) -> Result<bool, MemFault> {
    let count = m.i32(def.wrapping_add(WEAP_MAGAZINE_COUNT))?;
    if count <= 0 {
        let used = m.f32(weapon_body.wrapping_add(WEAPON_CHARGE_USED))?;
        if used > 0.0 {
            m.set_f32(weapon_body.wrapping_add(WEAPON_CHARGE_USED), 0.0)?;
            return Ok(true);
        }
        return Ok(false);
    }
    let block = m.u32(def.wrapping_add(WEAP_MAGAZINE_BLOCK))?;
    let mut changed = false;
    for i in 0..(count as u32).min(MAX_MAGAZINES) {
        let max = m.u16(
            block
                .wrapping_add(i * WEAP_MAGAZINE_STRIDE)
                .wrapping_add(WEAP_RESERVE_MAX),
        )? as i16;
        let at =
            weapon_body.wrapping_add(WEAPON_MAGAZINES + i * MAGAZINE_STRIDE + MAGAZINE_RESERVE);
        if max > 0 && (m.u16(at)? as i16) < max {
            m.set_u16(at, max as u16)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// The weapon definition (`weap` metadata) of a weapon object.
pub fn weapon_definition(m: &dyn GuestMemory, weapon: u32) -> Result<Option<(u32, u32)>, MemFault> {
    let Some(h) = DatumHandle::from_raw(weapon) else {
        return Ok(None);
    };
    let Some((kind, body)) = objects::get(m, h)? else {
        return Ok(None);
    };
    if kind != objects::WEAPON {
        return Ok(None);
    }
    let Some(def) = body.definition(m)? else {
        return Ok(None);
    };
    let meta = guest_layout::view::Tags.get(m, def, guest_layout::tags::GroupTag::from_chars(*b"weap"))?;
    Ok(meta.map(|d| (body.addr, d)))
}

/// Our skulls on in this game, by bit `n - COUNT`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Active(pub u32);

impl Active {
    pub fn of(set: &[usize]) -> Self {
        Active(
            set.iter()
                .filter(|i| skulls::is_ours(**i))
                .fold(0, |b, i| b | 1 << (i - COUNT)),
        )
    }
    pub fn has(self, skull: usize) -> bool {
        skulls::is_ours(skull) && self.0 & (1 << (skull - COUNT)) != 0
    }
    pub fn indices(self) -> Vec<usize> {
        (COUNT..skulls::TOTAL).filter(|i| self.has(*i)).collect()
    }
    pub fn names(self) -> String {
        let v: Vec<&str> = self.indices().iter().map(|i| SKULLS[*i].label).collect();
        if v.is_empty() {
            "none".into()
        } else {
            v.join(", ")
        }
    }
}

/// What the tick's effects did, for the once-a-game lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Effects {
    pub kills: Vec<u32>,
    pub lines: Vec<String>,
}

/// Applies `active`'s per-unit effects to the local players' units, once
/// a tick. `falls` is each biped's last tick.
pub fn apply_units(
    m: &mut dyn GuestMemory,
    active: Active,
    units: &[LocalUnit],
    falls: &mut HashMap<u32, Fall>,
) -> Result<Effects, MemFault> {
    let mut fx = Effects::default();
    for u in units {
        let b = u.body;
        if active.has(INVINCIBLE) {
            let f = m.u16(b.wrapping_add(BODY_DAMAGE_FLAGS))?;
            if f & CANNOT_TAKE_DAMAGE == 0 {
                m.set_u16(b.wrapping_add(BODY_DAMAGE_FLAGS), f | CANNOT_TAKE_DAMAGE)?;
            }
        }
        if active.has(UNLIMITED_GRENADES) {
            top_grenades(m, b)?;
        }
        if active.has(UNLIMITED_AMMO) {
            for w in weapons_of(m, b)? {
                if let Some((wb, def)) = weapon_definition(m, w)? {
                    refill(m, wb, def)?;
                }
            }
        }
        if u.kind != objects::BIPED || m.u32(b.wrapping_add(BODY_PARENT))? != u32::MAX {
            falls.remove(&u.unit);
            continue;
        }
        let mode = m.u8(b.wrapping_add(BIPED_MODE))?;
        let airborne = mode == MODE_AIRBORNE;
        let vz_at = b.wrapping_add(BODY_VELOCITY + 8);
        let vz = m.f32(vz_at)?;
        if active.has(FALL_DAMAGE)
            && !active.has(INVINCIBLE)
            && let Some(last) = falls.get(&u.unit)
            && let Some(impact) = landed(*last, airborne)
        {
            let damage = fall_damage(impact);
            if damage > 0.0 {
                let shield = m.f32(b.wrapping_add(BODY_SHIELD))?;
                let health = m.f32(b.wrapping_add(BODY_HEALTH))?;
                let (s, h, dead) = take(shield, health, damage);
                m.set_f32(b.wrapping_add(BODY_SHIELD), s)?;
                m.set_f32(b.wrapping_add(BODY_HEALTH), h)?;
                fx.lines.push(format!(
                    "skulls: Fall Damage: player {} landed at {impact:.2} wu/s: {damage:.2} \
                     vitality (shield {shield:.2} -> {s:.2}, health {health:.2} -> {h:.2}){}",
                    u.user,
                    if dead { "; killed" } else { "" }
                ));
                if dead {
                    let f = m.u16(b.wrapping_add(BODY_DAMAGE_FLAGS))?;
                    m.set_u16(b.wrapping_add(BODY_DAMAGE_FLAGS), f | KILL)?;
                    fx.kills.push(u.unit);
                }
            }
        }
        let mut vz_now = vz;
        if active.has(ACROPHOBIA) && airborne {
            let aim_z = m.f32(b.wrapping_add(UNIT_AIM + 8))?;
            let crouch = m.f32(b.wrapping_add(BIPED_CROUCH))?;
            if let Some(v) = fly(vz, aim_z, crouch) {
                m.set_f32(vz_at, v)?;
                vz_now = v;
            }
        }
        falls.insert(
            u.unit,
            Fall {
                airborne,
                vz: vz_now,
            },
        );
    }
    Ok(fx)
}

// ---- the debug aid ----

/// `H2_SKULLS_REVEAL`'s line for `spot`, with each local player's distance.
pub fn reveal_line(spot: &Spot, state: &str, players: &[(i16, [f32; 3])]) -> String {
    let [x, y, z] = spot.at;
    let dist: Vec<String> = players
        .iter()
        .map(|(u, p)| {
            let d = ((p[0] - x).powi(2) + (p[1] - y).powi(2) + (p[2] - z).powi(2)).sqrt();
            format!("player {u} {d:.1} wu away")
        })
        .collect();
    format!(
        "skulls reveal: {} on {} (bsp {}) at ({x:.2}, {y:.2}, {z:.2}) [{}]: {}; {}",
        SKULLS[spot.skull].label,
        spot.map,
        spot.bsp,
        spot.hint,
        state,
        if dist.is_empty() {
            "no local player".into()
        } else {
            dist.join(", ")
        }
    )
}

// ---- the run's state ----

/// Our skulls on for the campaign game being played (set at its start).
static CAMPAIGN: AtomicU32 = AtomicU32::new(0);

/// The campaign start's set of ours (`skulls`' start hook; empty for
/// anything but a campaign).
pub fn set_campaign(set: &[usize]) {
    let a = Active::of(set);
    CAMPAIGN.store(a.0, Ordering::SeqCst);
    if let Some(s) = SHARED.get() {
        s.campaign_started(a);
    }
}

/// Our skulls on in the campaign now.
pub fn campaign() -> Active {
    Active(CAMPAIGN.load(Ordering::SeqCst))
}

/// A guest call to make at the top of the next tick.
#[derive(Clone, Debug, PartialEq)]
pub enum Work {
    /// Create skull `skull` from weapon tag `tag` at `at`.
    Spawn {
        skull: usize,
        tag: u32,
        at: [f32; 3],
    },
    /// Show skull `skull`'s found message to local user `user`.
    Message { skull: usize, user: i16 },
    /// Remove the object `datum` as `object_destroy` does.
    Remove { datum: u32 },
    /// Let the game see a unit whose kill bit we set.
    Wake { datum: u32 },
}

/// The level being played, as far as our skull goes.
#[derive(Debug, Default)]
struct Level {
    /// (map, scenario datum): a new one starts over.
    key: Option<(String, u32)>,
    spot: Option<&'static Spot>,
    tag: Option<u32>,
    object: Option<u32>,
    uncollected: bool,
    queued: bool,
}

/// Finds a map's host file by the active slot's VFS device path.
pub type MapLocator = Box<dyn Fn(&str, Option<&str>) -> Option<vfs::HostFile> + Send + Sync>;

/// The run's state for our skulls.
pub struct Custom {
    store: Arc<Store>,
    telemetry: Option<Arc<crate::telemetry::Telemetry>>,
    maps: Option<MapLocator>,
    reveal: bool,
    edited_mount: AtomicBool,
    level: Mutex<Level>,
    work: Mutex<Vec<Work>>,
    reentry: AtomicBool,
    gravity: Mutex<Gravity>,
    /// The variant options' last decision, for the log.
    variant: Mutex<Option<crate::variant_ext::Decision>>,
    falls: Mutex<HashMap<u32, Fall>>,
    /// Bits of the skulls whose effect has said it acts this game.
    said: AtomicU32,
    /// Picked up this game (on until the next start).
    found: AtomicU32,
    /// Campaign ticks seen (for the reveal's pace).
    ticks: AtomicU32,
}

impl Custom {
    pub fn new(
        store: Arc<Store>,
        telemetry: Option<Arc<crate::telemetry::Telemetry>>,
        maps: Option<MapLocator>,
        reveal: bool,
    ) -> Self {
        Custom {
            store,
            telemetry,
            maps,
            reveal,
            edited_mount: AtomicBool::new(false),
            level: Mutex::default(),
            work: Mutex::default(),
            reentry: AtomicBool::new(false),
            gravity: Mutex::default(),
            variant: Mutex::default(),
            falls: Mutex::default(),
            said: AtomicU32::new(0),
            found: AtomicU32::new(0),
            ticks: AtomicU32::new(0),
        }
    }

    fn campaign_started(&self, a: Active) {
        self.said.store(0, Ordering::SeqCst);
        self.found.store(0, Ordering::SeqCst);
        if let Some(t) = &self.telemetry {
            t.set_campaign_custom_skulls(live_proto::skulls::normalize(
                a.indices().iter().map(|i| *i as u8),
            ));
        }
        if a.0 != 0 {
            eprintln!("skulls: ours on this campaign: {}", a.names());
        }
    }

    /// What acts now: the campaign's set plus what was found this game; in
    /// multiplayer only Low Gravity, if the host's set has it.
    pub fn active(&self, mode: Option<u32>) -> Active {
        match mode {
            Some(skulls::MODE_CAMPAIGN) => Active(campaign().0 | self.found.load(Ordering::SeqCst)),
            Some(crate::skulls_mp::MODE_MULTIPLAYER) => {
                let on = crate::skulls_mp::shared().is_some_and(|s| s.active(LOW_GRAVITY));
                Active::of(if on { &[LOW_GRAVITY] } else { &[] })
            }
            _ => Active::default(),
        }
    }

    /// Logs the variant options' decision when it changes.
    fn variant_said(&self, d: &crate::variant_ext::Decision) {
        let mut last = self.variant.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_ref() != Some(d) {
            if last.is_some() || *d != crate::variant_ext::Decision::Retail {
                eprintln!("{}", d.line());
            }
            *last = Some(*d);
        }
    }

    fn say_once(&self, skull: usize, line: impl FnOnce() -> String) {
        let bit = 1 << (skull - COUNT);
        if self.said.fetch_or(bit, Ordering::SeqCst) & bit == 0 {
            eprintln!("{}", line());
        }
    }

    fn queue(&self, w: Work) {
        self.work.lock().unwrap_or_else(|e| e.into_inner()).push(w);
    }

    /// The skull weapon's tag datum for `map`, from its file's tag index,
    /// if the file's scenario is the loaded one.
    fn skull_tag_for(&self, map: &str, path: Option<&str>, scenario: u32) -> Result<u32, String> {
        let locate = self.maps.as_ref().ok_or("no map locator (no VFS)")?;
        let file = locate(map, path).ok_or_else(|| format!("{map} has no host file"))?;
        let reader =
            std::fs::File::open(&file.path).map_err(|e| format!("{}: {e}", file.path.display()))?;
        let parsed = map::Map::from_region(std::io::BufReader::new(reader), file.offset, file.len)
            .map_err(|e| format!("{}: {e}", file.path.display()))?;
        let file_scenario = parsed.scenario().map(|t| t.id.0);
        if file_scenario != Some(scenario) {
            return Err(format!(
                "{} is not the loaded {map} (scenario {file_scenario:x?}, loaded {scenario:#x})",
                file.path.display()
            ));
        }
        skull_tag(
            parsed
                .tags()
                .iter()
                .filter_map(|t| Some((t.id.0, t.group.0, t.name.as_deref()?))),
        )
        .ok_or_else(|| format!("{map} has no skull weapon tag"))
    }

    /// Why a machine snapshot must wait: our guest calls are queued for the
    /// next tick or running.
    fn snapshot_live(&self) -> Option<String> {
        if self.reentry.load(Ordering::SeqCst) {
            return Some("our skulls' guest calls are running".into());
        }
        let queued = self.work.lock().unwrap_or_else(|e| e.into_inner()).len();
        (queued != 0).then(|| format!("{queued} of our skulls' guest calls are queued"))
    }

    /// The run's state as a machine snapshot keeps it
    /// (docs/runtime/snapshots.md): the campaign's set, what was said and
    /// found this game, the tick count, the level's skull and the effects'
    /// gravity and falls. The variant decision is only for the log.
    fn save_state(&self) -> Vec<u8> {
        use snapshot::codec::Writer;
        let opt = |w: &mut Writer, v: Option<u32>| {
            w.bool(v.is_some()).u32(v.unwrap_or(0));
        };
        let w = &mut Writer::new();
        w.u32(CAMPAIGN.load(Ordering::SeqCst))
            .u32(self.said.load(Ordering::SeqCst))
            .u32(self.found.load(Ordering::SeqCst))
            .u32(self.ticks.load(Ordering::SeqCst));
        {
            let level = self.level.lock().unwrap_or_else(|e| e.into_inner());
            match &level.key {
                Some((map, scenario)) => w.bool(true).str(map).u32(*scenario),
                None => w.bool(false),
            };
            let spot = level
                .spot
                .and_then(|spot| SPOTS.iter().position(|s| std::ptr::eq(s, spot)));
            opt(w, spot.map(|i| i as u32));
            opt(w, level.tag);
            opt(w, level.object);
            w.bool(level.uncollected).bool(level.queued);
        }
        let g = *self.gravity.lock().unwrap_or_else(|e| e.into_inner());
        opt(w, g.written.map(f32::to_bits));
        opt(w, g.game.map(f32::to_bits));
        let mut falls: Vec<(u32, Fall)> = self
            .falls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect();
        falls.sort_by_key(|(k, _)| *k);
        w.list(&falls, |w, (k, f)| {
            w.u32(*k).bool(f.airborne).u32(f.vz.to_bits());
        });
        std::mem::take(w).into_bytes()
    }

    /// Puts [`Self::save_state`]'s bytes back; nothing changes unless all
    /// of them decode.
    fn load_state(&self, bytes: &[u8]) -> Result<(), String> {
        use snapshot::codec::Reader;
        let r = &mut Reader::new(bytes, "skulls-custom");
        let opt = |r: &mut Reader<'_>| -> Result<Option<u32>, snapshot::Error> {
            let some = r.bool()?;
            let v = r.u32()?;
            Ok(some.then_some(v))
        };
        let e = |e: snapshot::Error| e.to_string();
        let campaign = r.u32().map_err(e)?;
        let said = r.u32().map_err(e)?;
        let found = r.u32().map_err(e)?;
        let ticks = r.u32().map_err(e)?;
        let key = if r.bool().map_err(e)? {
            Some((r.str().map_err(e)?.to_owned(), r.u32().map_err(e)?))
        } else {
            None
        };
        let spot = match opt(r).map_err(e)? {
            Some(i) => Some(SPOTS.get(i as usize).ok_or("no such skull spot")?),
            None => None,
        };
        let level = Level {
            key,
            spot,
            tag: opt(r).map_err(e)?,
            object: opt(r).map_err(e)?,
            uncollected: r.bool().map_err(e)?,
            queued: r.bool().map_err(e)?,
        };
        let gravity = Gravity {
            written: opt(r).map_err(e)?.map(f32::from_bits),
            game: opt(r).map_err(e)?.map(f32::from_bits),
        };
        let falls = r
            .list(9, |r| {
                let datum = r.u32()?;
                let airborne = r.bool()?;
                let vz = f32::from_bits(r.u32()?);
                Ok((datum, Fall { airborne, vz }))
            })
            .map_err(e)?;
        r.finish().map_err(e)?;

        CAMPAIGN.store(campaign, Ordering::SeqCst);
        if let Some(t) = &self.telemetry {
            t.set_campaign_custom_skulls(live_proto::skulls::normalize(
                Active(campaign).indices().iter().map(|i| *i as u8),
            ));
        }
        self.said.store(said, Ordering::SeqCst);
        self.found.store(found, Ordering::SeqCst);
        self.ticks.store(ticks, Ordering::SeqCst);
        *self.level.lock().unwrap_or_else(|e| e.into_inner()) = level;
        *self.gravity.lock().unwrap_or_else(|e| e.into_inner()) = gravity;
        *self.falls.lock().unwrap_or_else(|e| e.into_inner()) = falls.into_iter().collect();
        self.work.lock().unwrap_or_else(|e| e.into_inner()).clear();
        Ok(())
    }

    /// After each tick: the effects, then the hidden skull of the level.
    fn after_tick(&self, rt: &Arc<runtime::Runtime>) {
        let mut mem = rt.memory();
        let variant = crate::variant_ext::in_game(&*mem);
        self.variant_said(&variant);
        let m: &mut dyn GuestMemory = &mut *mem;
        let globals = m.u32(GAME_GLOBALS).unwrap_or(0);
        if globals == 0 {
            return;
        }
        let mode = m.u32(globals.wrapping_add(GLOBALS_MODE)).ok();
        let active = self.active(mode);
        // Gravity, whatever the players: Low Gravity times the variant's.
        let skull = if active.has(LOW_GRAVITY) {
            GRAVITY_SCALE
        } else {
            1.0
        };
        let scale = skull * variant.gravity();
        if let Ok(pc) = m.u32(PHYSICS_CONSTANTS)
            && pc != 0
            && let Ok(now) = m.f32(pc)
        {
            let mut g = self.gravity.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(v) = gravity_step(scale, now, &mut g) {
                let _ = m.set_f32(pc, v);
                if active.has(LOW_GRAVITY) {
                    self.say_once(LOW_GRAVITY, || {
                        format!(
                            "skulls: Low Gravity: gravity {now} -> {v} (physics constants {pc:#x})"
                        )
                    });
                }
                if variant.gravity() != 1.0 {
                    eprintln!(
                        "variant options: gravity {now} -> {v} (variant {}%, Low Gravity {})",
                        (variant.gravity() * 100.0).round(),
                        if skull != 1.0 { "on" } else { "off" }
                    );
                }
            }
        }
        // The variant's Unlimited Grenades: the host's (or a lone
        // machine's) units, every player's.
        if mode == Some(crate::skulls_mp::MODE_MULTIPLAYER)
            && variant.unlimited_grenades()
            && let Err(e) = variant_grenades(m)
        {
            self.say_once(UNLIMITED_GRENADES, || {
                format!("variant options: unlimited grenades could not read the players: {e:?}")
            });
        }
        if mode != Some(skulls::MODE_CAMPAIGN) {
            return;
        }
        let units = local_units(m).unwrap_or_default();
        if active.0 != 0 {
            let mut falls = self.falls.lock().unwrap_or_else(|e| e.into_inner());
            match apply_units(m, active, &units, &mut falls) {
                Ok(fx) => {
                    for line in fx.lines {
                        eprintln!("{line}");
                    }
                    for k in fx.kills {
                        self.queue(Work::Wake { datum: k });
                    }
                }
                Err(e) => self.say_once(ACROPHOBIA, || {
                    format!("skulls: our skulls' effects could not read the players: {e:?}")
                }),
            }
            for s in active.indices() {
                if s != LOW_GRAVITY {
                    self.say_once(s, || {
                        format!(
                            "skulls: {} is acting on the local players this game",
                            SKULLS[s].label
                        )
                    });
                }
            }
        }
        // The hidden skull.
        let map = crate::telemetry::loaded_map_name(&mem).ok();
        let m: &mut dyn GuestMemory = &mut *mem;
        let scenario = guest_layout::view::Tags
            .scenario(m)
            .ok()
            .flatten()
            .map(|h| h.raw());
        let bsp = m
            .u16(globals.wrapping_add(GLOBALS_ACTIVE_BSP))
            .ok()
            .map(|v| v as i16);
        let tick = self.ticks.fetch_add(1, Ordering::Relaxed);
        let (Some(map), Some(scenario)) = (map, scenario) else {
            return;
        };
        let key = (map.clone(), scenario);
        let mut level = self.level.lock().unwrap_or_else(|e| e.into_inner());
        let new_level = level.key.as_ref() != Some(&key);
        // Who holds ours, with the object's parent and liveness.
        let mut picked = None;
        let mut alive = false;
        if let Some(obj) = level.object
            && let Some(h) = DatumHandle::from_raw(obj)
            && let Ok(Some((_, body))) = objects::get(m, h)
        {
            alive = !new_level;
            let parent = m.u32(body.addr.wrapping_add(BODY_PARENT)).ok();
            let held: Vec<(u32, Vec<u32>)> = units
                .iter()
                .map(|u| (u.unit, weapons_of(m, u.body).unwrap_or_default()))
                .collect();
            picked = held_by(obj, parent.filter(|p| *p != u32::MAX), &held);
        }
        let positions: Vec<(i16, [f32; 3])> = units
            .iter()
            .filter_map(|u| Some((u.user, m.vec3(u.body.wrapping_add(0x64)).ok()?)))
            .collect();
        let profiles = skulls::profiles_or_default(&*mem).0;
        let path = (new_level && self.edited_mount.load(Ordering::Acquire))
            .then(|| crate::map_slots::active_file(&*mem)).flatten();
        drop(mem);
        if new_level {
            *level = Level {
                key: Some(key),
                spot: spot_for(&map),
                ..Level::default()
            };
            if let Some(spot) = level.spot {
                level.uncollected = uncollected(&self.store, &profiles, spot.skull);
                match self.skull_tag_for(&map, path.as_deref(), scenario) {
                    Ok(t) => level.tag = Some(t),
                    Err(why) => eprintln!(
                        "skulls: {} is hidden on {map} but cannot be placed: {why}",
                        SKULLS[spot.skull].label
                    ),
                }
                eprintln!(
                    "skulls: {map} hides {} (bsp {}): {}",
                    SKULLS[spot.skull].label,
                    spot.bsp,
                    if level.uncollected {
                        "not yet collected by everyone here; it will be placed"
                    } else {
                        "already collected by every profile here; not placed"
                    }
                );
                if self.reveal {
                    eprintln!("{}", reveal_line(spot, "level loaded", &positions));
                }
            }
        }
        let Some(spot) = level.spot else { return };
        if let Some(i) = picked
            && let Some(obj) = level.object.take()
        {
            let u = units[i];
            self.found
                .fetch_or(1 << (spot.skull - COUNT), Ordering::SeqCst);
            level.uncollected = false;
            eprintln!(
                "skulls: ==== FOUND {} on {} by player {} (object {obj:#x}); on for the rest of this level ====",
                SKULLS[spot.skull].label, spot.map, u.user
            );
            for line in skulls::record_pickup(&self.store, &profiles, spot.skull) {
                eprintln!("{line}");
            }
            self.queue(Work::Message {
                skull: spot.skull,
                user: u.user,
            });
            self.queue(Work::Remove { datum: obj });
            return;
        }
        if !alive && level.object.is_some() && !level.queued {
            // A revert or a BSP change took it: place it again.
            eprintln!(
                "skulls: {} is gone from {} (a checkpoint revert or BSP change); placing it again when its BSP is in",
                SKULLS[spot.skull].label, spot.map
            );
            level.object = None;
        }
        if !level.queued && should_spawn(true, spot, bsp, level.uncollected, level.tag, alive) {
            level.queued = true;
            self.queue(Work::Spawn {
                skull: spot.skull,
                tag: level.tag.unwrap_or(u32::MAX),
                at: spot.at,
            });
        }
        if self.reveal && tick.is_multiple_of(REVEAL_EVERY) {
            let state = if !level.uncollected {
                "collected".to_string()
            } else if level.object.is_some() {
                "placed".to_string()
            } else if bsp != Some(spot.bsp) {
                format!("waiting for bsp {} (bsp {bsp:?} is in)", spot.bsp)
            } else {
                "not placed".to_string()
            };
            eprintln!("{}", reveal_line(spot, &state, &positions));
        }
    }

    fn spawned(&self, skull: usize, at: [f32; 3], datum: u32) {
        let mut level = self.level.lock().unwrap_or_else(|e| e.into_inner());
        level.queued = false;
        if datum == u32::MAX {
            // No retry: it would fail the same way every tick.
            level.tag = None;
            eprintln!(
                "skulls: object_new refused {}; not placed this level",
                SKULLS[skull].label
            );
            return;
        }
        level.object = Some(datum);
        let [x, y, z] = at;
        eprintln!(
            "skulls: placed {} at ({x:.2}, {y:.2}, {z:.2}) as object {datum:#x}",
            SKULLS[skull].label
        );
    }
}

async fn run_work(
    cx: guest_layout::entry::Cx,
    s: Arc<Custom>,
    work: Vec<Work>,
    scratch: u32,
) -> Result<Ret, guest_layout::entry::EntryError> {
    for w in work {
        match w {
            Work::Spawn { skull, tag, at } => {
                cx.call(
                    GuestCall::new(PLACEMENT_NEW)
                        .esi(scratch)
                        .args([tag, u32::MAX, 0]),
                )
                .await?;
                cx.write(|m| {
                    for (i, v) in at.iter().enumerate() {
                        m.set_f32(scratch + PLACEMENT_POSITION + 4 * i as u32, *v)?;
                    }
                    Ok::<_, MemFault>(())
                })?;
                let datum = cx
                    .call(GuestCall::new(OBJECT_NEW).args([scratch]))
                    .await?
                    .eax;
                s.spawned(skull, at, datum);
            }
            Work::Message { skull, user } => {
                cx.call(
                    GuestCall::new(HUD_MESSAGE)
                        .edi(crate::skulls_menu::found_id(skull))
                        .args([user as u32]),
                )
                .await?;
            }
            Work::Remove { datum } => {
                let refused = cx
                    .call(GuestCall::new(OBJECT_DESTROY_REFUSES).args([datum]))
                    .await?
                    .eax
                    & 0xff;
                if refused == 0 {
                    cx.call(GuestCall::new(OBJECT_DELETE).args([datum])).await?;
                    eprintln!("skulls: removed the found skull (object {datum:#x})");
                } else {
                    eprintln!("skulls: the game would not remove object {datum:#x}; left as it is");
                }
            }
            Work::Wake { datum } => {
                cx.call(GuestCall::new(OBJECT_WAKE).eax(datum)).await?;
            }
        }
    }
    // Now the tick itself: its chain runs again, this time continuing.
    s.reentry.store(true, Ordering::SeqCst);
    let r = cx
        .call(GuestCall::new(guest_layout::points::GAME_TICK.base))
        .await;
    s.reentry.store(false, Ordering::SeqCst);
    r?;
    Ok(Ret::new(0, 0))
}

static SHARED: OnceLock<Arc<Custom>> = OnceLock::new();

/// The feature: a `before` on the tick for our guest calls, and an `after`
/// for the effects and the hidden skull.
pub struct SkullsCustom(pub Arc<Custom>);

impl hooks::Feature for SkullsCustom {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Our own skulls, hidden in the campaign")
            .doc("docs/game/skulls.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::points::GAME_TICK;
        let scratch = b.guest_alloc(0x1000, runtime::guest::Permissions::RW)?;
        let _ = PLACEMENT_SIZE;
        let s = Arc::clone(&self.0);
        b.raw()
            .before(&GAME_TICK, "our skulls' calls", move |c| {
                if s.reentry.load(Ordering::SeqCst) {
                    return Ok(hooks::Flow::Continue);
                }
                let work = std::mem::take(&mut *s.work.lock().unwrap_or_else(|e| e.into_inner()));
                if work.is_empty() {
                    return Ok(hooks::Flow::Continue);
                }
                let s = Arc::clone(&s);
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(run_work(cx, s, work, scratch))
                })
            })
            // Inside agent calls, before the other tick hooks.
            .first()
            .after_feature(agent::sim::calls::FEATURE);
        let s = Arc::clone(&self.0);
        b.raw()
            .after(&GAME_TICK, "our skulls", move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                s.after_tick(&rt);
                Ok(())
            })
            // The Halo 1 weapon HUD samples after our skulls' writes.
            .after_feature(crate::h1_hud_text::FEATURE);
        let (live, save, load) = (
            Arc::clone(&self.0),
            Arc::clone(&self.0),
            Arc::clone(&self.0),
        );
        b.snapshot(hooks::SnapshotPolicy::section_forbid_while(
            move || live.snapshot_live(),
            move || save.save_state(),
            move |bytes, _| load.load_state(bytes),
        ));
        Ok(())
    }
}

/// Adds our skulls to `registry`. Says why when left out.
pub fn install(
    registry: &mut hooks::Registry,
    store: Option<Arc<Store>>,
    telemetry: Option<Arc<crate::telemetry::Telemetry>>,
    maps: Option<MapLocator>,
) -> anyhow::Result<bool> {
    install_for_maps(registry, store, telemetry, maps, None)
}

pub fn install_for_maps(
    registry: &mut hooks::Registry,
    store: Option<Arc<Store>>,
    telemetry: Option<Arc<crate::telemetry::Telemetry>>,
    maps: Option<MapLocator>,
    sources: Option<&crate::loaded_map::EditorSources>,
) -> anyhow::Result<bool> {
    let Some(store) = store else {
        eprintln!("skulls-custom: off: no settings store (a headless run)");
        return Ok(false);
    };
    let reveal = std::env::var("H2_SKULLS_REVEAL").is_ok_and(|v| v == "1");
    let shared = Arc::new(Custom::new(store, telemetry, maps, reveal));
    let added = registry
        .add(&SkullsCustom(Arc::clone(&shared)))
        .map_err(|e| anyhow::anyhow!("skulls-custom: {e}"))?;
    if !added {
        eprintln!("skulls-custom: off: switched off (H2_FEATURES)");
        return Ok(false);
    }
    if let Some(sources) = sources {
        let state = Arc::downgrade(&shared);
        sources.on_change(move |_| {
            if let Some(s) = state.upgrade() {
                s.edited_mount.store(true, Ordering::Release);
                *s.level.lock().unwrap_or_else(|e| e.into_inner()) = Level::default();
                s.work.lock().unwrap_or_else(|e| e.into_inner()).clear();
            }
        });
    }
    if reveal {
        eprintln!(
            "skulls-custom: H2_SKULLS_REVEAL=1: logging our skulls' spots every {REVEAL_EVERY} ticks"
        );
        for s in &SPOTS {
            eprintln!("{}", reveal_line(s, "at boot", &[]));
        }
    }
    let _ = SHARED.set(shared);
    Ok(true)
}

/// The install line, once `plan` is in.
pub fn report(plan: &hooks::Plan, installed: &hooks::Installed) -> String {
    if let Some(why) = installed.off_reason(FEATURE) {
        return format!(
            "skulls-custom: NOT installed: {why}; our skulls are neither placed nor applied"
        );
    }
    if !plan.is_on(FEATURE) {
        return "skulls-custom: NOT installed: not in the feature plan".into();
    }
    let placed = plan
        .chains
        .values()
        .find(|c| {
            std::ptr::eq(c.point, &guest_layout::points::GAME_TICK)
                && c.hooks().any(|h| h.feature == FEATURE)
        })
        .map(|c| (c.addr, installed.placed.contains(&c.addr)));
    let levels: Vec<String> = SPOTS
        .iter()
        .map(|s| format!("{} on {}", SKULLS[s.skull].label, ABOUT[s.skull].level))
        .collect();
    match placed {
        Some((addr, true)) => format!(
            "skulls-custom: hooks installed via feature plan at {addr:#x} (tick: calls, effects, hidden skulls); hidden: {}",
            levels.join(", ")
        ),
        Some((addr, false)) => {
            format!("skulls-custom: NOT installed: the tick hook at {addr:#x} was not placed")
        }
        None => "skulls-custom: NOT installed: no tick hook in the plan".into(),
    }
}

/// A profile value written straight, for tests and tools: collected and on.
pub fn collect(store: &Store, profile: &str, skull: usize) -> Option<String> {
    store
        .profile_set(profile, SKULLS.get(skull)?.key, Value::Bool(true))
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests;
