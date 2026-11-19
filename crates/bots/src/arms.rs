//! What a bot carries and what lies about (docs/game/bots.md, section 3.6):
//! the held and the other weapon's ammunition, the grenades, the weapons on
//! the ground, and the pure decision of what to do about them ([`plan`]).
//!
//! The fields were traced offline from the XBE (no game run); section 3.6
//! has the addresses and how. In short:
//!
//! - unit `+0x212` the held weapon's slot, `+0x218` four weapon datums,
//!   `+0x23e` two grenade counts;
//! - weapon `+0x184` the battery used (0 fresh, 1 dead), `+0x228` the
//!   magazine states, `0x10` each: `+2` (`W+0x22a`) the reserve, `+4`
//!   (`W+0x22c`) the rounds loaded;
//! - `weap` definition `+0x2c0`/`+0x2c4` the magazine block (`0x5c` each:
//!   `+0xa` the rounds a magazine holds, `+0xc` the most in reserve),
//!   `+0x2d0`/`+0x2d4` the barrel block (`0xec` each: `+0xa8` the battery
//!   used per shot).

use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::view::{Tags, objects};

use crate::brain::Vec3;

type R<T> = Result<T, MemFault>;

/// Unit `+0x212`: the held weapon's slot in [`UNIT_WEAPONS`] (`i8`, -1 for
/// none).
pub const UNIT_WEAPON_INDEX: u32 = 0x212;
/// Unit `+0x218`: the four weapon object datums.
pub const UNIT_WEAPONS: u32 = 0x218;
pub const UNIT_WEAPON_SLOTS: u32 = 4;
/// Unit `+0x23e`: the frag and plasma grenade counts (`i8` each).
pub const UNIT_GRENADES: u32 = 0x23e;

/// Weapon `+0x184`: the battery used, 0 to 1 (`f32`).
pub const WEAPON_AGE: u32 = 0x184;
/// Weapon `+0x228`: the magazine states, [`MAGAZINE_STATE_STRIDE`] each.
pub const WEAPON_MAGAZINES: u32 = 0x228;
pub const MAGAZINE_STATE_STRIDE: u32 = 0x10;
/// In a magazine state: the reserve and the rounds loaded (`i16`).
pub const MAGAZINE_RESERVE: u32 = 2;
pub const MAGAZINE_LOADED: u32 = 4;

/// `weap` `+0x2c0`: the magazine block's count, then its pointer.
pub const WEAP_MAGAZINES: u32 = 0x2c0;
pub const MAGAZINE_DEF_STRIDE: u32 = 0x5c;
/// In a magazine definition: rounds a magazine holds, most in reserve
/// (`i16`).
pub const MAGAZINE_DEF_LOADED_MAX: u32 = 0xa;
pub const MAGAZINE_DEF_RESERVE_MAX: u32 = 0xc;
/// `weap` `+0x2d0`: the barrel block's count, then its pointer.
pub const WEAP_BARRELS: u32 = 0x2d0;
pub const BARREL_DEF_STRIDE: u32 = 0xec;
/// In a barrel definition: the battery used per shot (`f32`).
pub const BARREL_DEF_USAGE: u32 = 0xa8;
/// Barrels looked at, at most (a weapon has one or two).
const MAX_BARRELS: u32 = 8;

/// A weapon's ammunition.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Ammo {
    /// Rounds: loaded of `capacity`, and `reserve` of `reserve_max`.
    Rounds {
        loaded: i16,
        capacity: i16,
        reserve: i16,
        reserve_max: i16,
    },
    /// A battery, 0 (dead) to 1 (full).
    Battery(f32),
    /// Neither: not a gun (the flag, the ball, the bomb), or not read.
    #[default]
    None,
}

/// Below this a battery counts as dead.
pub const BATTERY_DEAD: f32 = 0.02;
/// Reloads when calm with this little of a magazine left.
pub const RELOAD_BELOW: f32 = 0.35;
/// "Low": this little of all it can carry.
pub const LOW: f32 = 0.2;

impl Ammo {
    /// Nothing to shoot, loaded or not.
    pub fn dry(self) -> bool {
        match self {
            Ammo::Rounds {
                loaded, reserve, ..
            } => loaded <= 0 && reserve <= 0,
            Ammo::Battery(b) => b <= BATTERY_DEAD,
            Ammo::None => true,
        }
    }
    /// The magazine is empty but there are rounds to load.
    pub fn empty_magazine(self) -> bool {
        matches!(self, Ammo::Rounds { loaded, reserve, .. } if loaded <= 0 && reserve > 0)
    }
    /// Worth a reload: the magazine under [`RELOAD_BELOW`] with rounds to
    /// load.
    pub fn wants_reload(self) -> bool {
        match self {
            Ammo::Rounds {
                loaded,
                capacity,
                reserve,
                ..
            } => {
                reserve > 0
                    && loaded < capacity
                    && (loaded as f32) < RELOAD_BELOW * capacity.max(1) as f32
            }
            _ => false,
        }
    }
    /// How much of all it can carry is left, 0 to 1.
    pub fn fraction(self) -> f32 {
        match self {
            Ammo::Rounds {
                loaded,
                capacity,
                reserve,
                reserve_max,
            } => {
                let most = (capacity.max(0) as f32 + reserve_max.max(0) as f32).max(1.0);
                ((loaded.max(0) as f32 + reserve.max(0) as f32) / most).clamp(0.0, 1.0)
            }
            Ammo::Battery(b) => b.clamp(0.0, 1.0),
            Ammo::None => 0.0,
        }
    }
    pub fn low(self) -> bool {
        self.fraction() < LOW
    }
    /// A gun at all (rounds or a battery).
    pub fn gun(self) -> bool {
        !matches!(self, Ammo::None)
    }
}

/// One weapon: its definition (the `weap` tag datum) and its ammunition.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Gun {
    pub def: u32,
    pub ammo: Ammo,
}

/// What the bot carries.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Arms {
    /// The weapon in hand.
    pub held: Option<Gun>,
    /// The other one (multiplayer carries two).
    pub other: Option<Gun>,
    /// Frag and plasma grenades.
    pub grenades: [i8; 2],
}

impl Arms {
    pub fn has_grenades(&self) -> bool {
        self.grenades.iter().any(|&g| g > 0)
    }
    /// Whether `def` is a weapon it already carries.
    pub fn carries(&self, def: u32) -> bool {
        [self.held, self.other]
            .iter()
            .flatten()
            .any(|g| g.def == def)
    }
}

/// A weapon on the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pickup {
    /// The weapon object's datum.
    pub id: u32,
    pub position: Vec3,
    pub gun: Gun,
}

impl Pickup {
    /// Something to take: a gun with something in it.
    pub fn useful(&self) -> bool {
        self.gun.ammo.gun() && !self.gun.ammo.dry()
    }
}

// ---- the decision ----

/// What to do about weapons this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kit {
    /// Nothing: fight or roam as usual.
    Fine,
    /// Press reload.
    Reload,
    /// Press switch weapons.
    Switch,
    /// Go and take the weapon on the ground: walk over it (same kind: its
    /// ammunition is taken) or hold the action button on it (another kind).
    Seek(Pickup),
    /// Nothing to shoot: close in and melee.
    Melee,
}

/// What [`plan`] looks at.
#[derive(Clone, Copy, Debug)]
pub struct Situation<'a> {
    pub arms: &'a Arms,
    pub at: Vec3,
    /// The distance to the enemy in view, if any.
    pub enemy: Option<f32>,
    /// No enemy in view for a while: safe to reload, worth a detour.
    pub calm: bool,
    pub pickups: &'a [Pickup],
}

/// Goes for weapons on the ground within this (world units).
pub const SEEK_RANGE: f32 = 30.0;
/// ...or, merely low and calm, within this.
pub const TOP_UP_RANGE: f32 = 12.0;
/// Rather than walk away, goes for the enemy with no ammunition this close.
pub const MELEE_CHASE: f32 = 2.5;

fn dist(a: Vec3, b: Vec3) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// The decision, pure. In order:
///
/// 1. the held gun is dry: switch to the other if it has ammunition; else
///    melee an enemy within [`MELEE_CHASE`]; else go for a pickup within
///    [`SEEK_RANGE`]; else melee whatever comes;
/// 2. an empty magazine with rounds to load: reload, enemy or not;
/// 3. calm with a magazine under [`RELOAD_BELOW`]: reload;
/// 4. calm with both weapons low: top up from a pickup within
///    [`TOP_UP_RANGE`];
/// 5. otherwise fine.
///
/// `skip` rules out pickups (those the bot could not reach).
pub fn plan(s: &Situation<'_>, skip: impl Fn(&Pickup) -> bool) -> Kit {
    let held = s.arms.held.map_or(Ammo::None, |g| g.ammo);
    let other = s.arms.other.map(|g| g.ammo);
    let nearest = |range: f32, want: &dyn Fn(&Pickup) -> bool| {
        s.pickups
            .iter()
            .filter(|&p| p.useful() && want(p) && !skip(p))
            .map(|p| (dist(p.position, s.at), *p))
            .filter(|(d, _)| *d < range)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    };
    if held.dry() {
        if other.is_some_and(|o| !o.dry()) {
            return Kit::Switch;
        }
        if s.enemy.is_some_and(|d| d < MELEE_CHASE) {
            return Kit::Melee;
        }
        if let Some(p) = nearest(SEEK_RANGE, &|_| true) {
            return Kit::Seek(p);
        }
        return if s.enemy.is_some() {
            Kit::Melee
        } else {
            Kit::Fine
        };
    }
    if held.empty_magazine() {
        return Kit::Reload;
    }
    if s.calm && held.wants_reload() {
        return Kit::Reload;
    }
    if s.calm && held.low() && other.is_none_or(|o| o.low()) {
        // Same kind tops up by walking over it; any useful gun will do.
        if let Some(p) = nearest(TOP_UP_RANGE, &|_| true) {
            return Kit::Seek(p);
        }
    }
    Kit::Fine
}

// ---- reading it from the guest ----

/// A `weap` definition's ammunition layout, read from the tag.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct WeaponDef {
    /// The first magazine's rounds loaded and reserve maxima, if it has
    /// magazines.
    pub magazine: Option<(i16, i16)>,
    /// A barrel uses battery per shot.
    pub ages: bool,
}

impl WeaponDef {
    /// Reads the definition at `weap` (the tag's metadata).
    pub fn read(m: &dyn GuestMemory, weap: u32) -> R<WeaponDef> {
        let mags = m.i32(weap + WEAP_MAGAZINES)?;
        let magazine = if mags > 0 {
            let first = m.u32(weap + WEAP_MAGAZINES + 4)?;
            Some((
                m.u16(first + MAGAZINE_DEF_LOADED_MAX)? as i16,
                m.u16(first + MAGAZINE_DEF_RESERVE_MAX)? as i16,
            ))
        } else {
            None
        };
        let barrels = m.i32(weap + WEAP_BARRELS)?.clamp(0, MAX_BARRELS as i32) as u32;
        let base = if barrels > 0 {
            m.u32(weap + WEAP_BARRELS + 4)?
        } else {
            0
        };
        let mut ages = false;
        for i in 0..barrels {
            if m.f32(base + i * BARREL_DEF_STRIDE + BARREL_DEF_USAGE)? > 0.0 {
                ages = true;
            }
        }
        Ok(WeaponDef { magazine, ages })
    }

    /// The ammunition of a weapon of this definition whose body is at
    /// `body`. A weapon that uses battery reads as one, as `0x101a10`
    /// sets it; one with magazines and no battery as rounds; one with
    /// neither is [`Ammo::None`].
    pub fn ammo(self, m: &dyn GuestMemory, body: u32) -> R<Ammo> {
        if self.ages {
            return Ok(Ammo::Battery(1.0 - m.f32(body + WEAPON_AGE)?));
        }
        let Some((capacity, reserve_max)) = self.magazine else {
            return Ok(Ammo::None);
        };
        let state = body + WEAPON_MAGAZINES;
        Ok(Ammo::Rounds {
            loaded: m.u16(state + MAGAZINE_LOADED)? as i16,
            capacity,
            reserve: m.u16(state + MAGAZINE_RESERVE)? as i16,
            reserve_max,
        })
    }
}

/// The gun whose object is `weapon`, if it is a live weapon.
pub fn gun(m: &dyn GuestMemory, weapon: DatumHandle) -> R<Option<(Gun, objects::Body)>> {
    let Some((kind, body)) = objects::get(m, weapon)? else {
        return Ok(None);
    };
    if kind != objects::WEAPON {
        return Ok(None);
    }
    let Some(def) = body.definition(m)? else {
        return Ok(None);
    };
    let Some(tag) = Tags.entry(m, def)? else {
        return Ok(None);
    };
    let weap = tag.address(m)?;
    if weap == 0 {
        return Ok(None);
    }
    let ammo = WeaponDef::read(m, weap)?.ammo(m, body.addr)?;
    Ok(Some((
        Gun {
            def: def.raw(),
            ammo,
        },
        body,
    )))
}

/// What the unit whose body is at `unit` carries.
pub fn read(m: &dyn GuestMemory, unit: u32) -> R<Arms> {
    let held_slot = m.u8(unit + UNIT_WEAPON_INDEX)? as i8;
    let mut arms = Arms {
        grenades: [
            m.u8(unit + UNIT_GRENADES)? as i8,
            m.u8(unit + UNIT_GRENADES + 1)? as i8,
        ],
        ..Arms::default()
    };
    for slot in 0..UNIT_WEAPON_SLOTS {
        let Some(h) = DatumHandle::from_raw(m.u32(unit + UNIT_WEAPONS + 4 * slot)?) else {
            continue;
        };
        let Some((g, _)) = gun(m, h)? else {
            continue;
        };
        if slot as i32 == i32::from(held_slot) {
            arms.held = Some(g);
        } else if arms.other.is_none() {
            arms.other = Some(g);
        }
    }
    Ok(arms)
}

/// Objects looked at, at most.
pub const OBJECT_LIMIT: i32 = 2048;

/// The weapons lying in the world: live weapon objects with no parent
/// (nobody holds them).
pub fn pickups(m: &dyn GuestMemory) -> R<Vec<Pickup>> {
    let Some(array) = objects::array(m)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (h, header) in array.live(m, OBJECT_LIMIT)? {
        if header.kind(m)? != objects::WEAPON || header.body(m)? == 0 {
            continue;
        }
        let Some((g, body)) = gun(m, h)? else {
            continue;
        };
        if body.parent(m)?.is_some() {
            continue;
        }
        out.push(Pickup {
            id: h.raw(),
            position: body.position(m)?,
            gun: g,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
