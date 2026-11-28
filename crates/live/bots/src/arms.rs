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
//!
//! The handling ([`Profile`], section 3.6 "Handling"), for the weapon's
//! [`Role`] and its [`preferred_range`]: `weap` `+0x1fe` zoom levels,
//! `+0x20c`/`+0x214` the aim assist's reach, `+0x2c8` the triggers (`+6`
//! the behaviour), and in the first barrel `+8` the rate, `+0x6a` the
//! projectiles per shot and `+0x8c` the projectile, whose `proj` `+0x17c`
//! is its speed.
//!
//! Weapon skill (`crate::weapons`, `crate::grenades`): unit `+0x213` the
//! left-hand weapon's slot when dual wielding, `+0x23c` the grenade type
//! selected.

use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::tags::GroupTag;
use guest_layout::view::{Tags, objects};

use crate::brain::Vec3;

type R<T> = Result<T, MemFault>;

/// Unit `+0x212`: the held weapon's slot in [`UNIT_WEAPONS`] (`i8`, -1 for
/// none).
pub const UNIT_WEAPON_INDEX: u32 = 0x212;
/// Unit `+0x218`: the four weapon object datums.
pub const UNIT_WEAPONS: u32 = 0x218;
pub const UNIT_WEAPON_SLOTS: u32 = 4;
/// Unit `+0x213`: the left-hand weapon's slot when dual wielding (`i8`,
/// -1 for none; docs/live/live-gamestate.md, `crates/extensions/game/src/touch_glue.rs`,
/// `0x186bd4`).
pub const UNIT_LEFT_WEAPON_INDEX: u32 = 0x213;
/// Unit `+0x23c`: the grenade type selected (`i8`, an index into
/// [`UNIT_GRENADES`]: 0 frag, 1 plasma; docs/live/live-gamestate.md).
pub const UNIT_GRENADE_SELECTED: u32 = 0x23c;
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

// The weapon's handling (section 3.6, "Handling"): what kind of gun it is,
// for the range it is good at, the trigger and the lead.

/// `weap` `+0x1fe`: the zoom levels (`i16`; the sniper and beam rifles 2,
/// the battle rifle, carbine and rocket launcher 1, others 0;
/// `boot::touch_glue`).
pub const WEAP_ZOOM_LEVELS: u32 = 0x1fe;
/// `weap` `+0x20c`, `+0x214`: the aim assist's `cone_range` and
/// `pull_range` (`f32`, world units; docs/game/aim-assist.md).
pub const WEAP_CONE_RANGE: u32 = 0x20c;
pub const WEAP_PULL_RANGE: u32 = 0x214;
/// `weap` `+0x2c8`: the triggers block's count, then its pointer (none on
/// the energy sword).
pub const WEAP_TRIGGERS: u32 = 0x2c8;
pub const TRIGGER_DEF_STRIDE: u32 = 0x40;
/// In a trigger definition: its behaviour (`u16`; 0 fires while held, 1, 4
/// and 5 once per pull, 2 and 3 charge).
pub const TRIGGER_DEF_BEHAVIOUR: u32 = 6;
/// In a barrel definition: the most rounds a second (`f32`).
pub const BARREL_DEF_RATE: u32 = 8;
/// In a barrel definition: projectiles per shot (`i16`).
pub const BARREL_DEF_PROJECTILES: u32 = 0x6a;
/// In a barrel definition: the projectile, a tag reference (group, then
/// datum at `+4`).
pub const BARREL_DEF_PROJECTILE: u32 = 0x8c;
/// `proj` `+0x17c`: the initial velocity (`f32`, world units a second).
pub const PROJ_INITIAL_VELOCITY: u32 = 0x17c;
/// `proj` `+0xe0`: maximum projectile travel (world units).
pub const PROJ_MAXIMUM_RANGE: u32 = 0xe0;
/// `proj` `+0x16c`: far end of the air damage falloff (world units).
pub const PROJ_AIR_DAMAGE_RANGE_END: u32 = 0x16c;
const PROJ: GroupTag = GroupTag::from_chars(*b"proj");

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
    /// Something to fire now, without a reload: rounds in the magazine or
    /// a live battery.
    pub fn ready(self) -> bool {
        match self {
            Ammo::Rounds { loaded, .. } => loaded > 0,
            Ammo::Battery(b) => b > BATTERY_DEAD,
            Ammo::None => false,
        }
    }
    /// The magazine's fill, 0 to 1 (a battery's charge; 0 for neither).
    pub fn magazine(self) -> f32 {
        match self {
            Ammo::Rounds {
                loaded, capacity, ..
            } => (loaded.max(0) as f32 / capacity.max(1) as f32).clamp(0.0, 1.0),
            Ammo::Battery(b) => b.clamp(0.0, 1.0),
            Ammo::None => 0.0,
        }
    }
}

/// How a trigger fires (`weap` `+0x2c8`, a trigger's `+6`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Trigger {
    /// Not read: the bot holds the trigger as before.
    #[default]
    Unknown,
    /// Fires while held (SMG, plasma rifle, needler).
    Automatic,
    /// Once per pull (magnum, battle rifle, carbine, shotgun, sniper,
    /// rocket launcher): it must be let go and pulled again.
    Single,
    /// Charges while held (plasma pistol); a quick pull fires at once.
    Charge,
    /// No triggers block at all (the energy sword).
    Absent,
}

impl Trigger {
    /// A trigger definition's behaviour value.
    pub fn from_behaviour(b: u16) -> Trigger {
        match b {
            0 => Trigger::Automatic,
            1 | 4 | 5 => Trigger::Single,
            2 | 3 => Trigger::Charge,
            _ => Trigger::Unknown,
        }
    }
}

/// A weapon's handling, read from its `weap` tag (section 3.6,
/// "Handling"). All zero (and [`Trigger::Unknown`]) when not read: then
/// every rule falls back to the bot's old, weapon-blind behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Profile {
    pub trigger: Trigger,
    pub dual: Option<DualGun>,
    /// Zoom levels (`+0x1fe`).
    pub zoom: i16,
    /// The aim assist's reach (`+0x20c`, `+0x214`), world units.
    pub cone_range: f32,
    pub pull_range: f32,
    /// The first barrel's most rounds a second, 0 unknown.
    pub rate: f32,
    /// The first barrel's projectiles per shot, 0 unknown.
    pub pellets: i16,
    /// The first barrel's projectile's initial speed (world units a
    /// second), 0 unknown.
    pub shot_speed: f32,
    /// The first barrel's effective projectile range, 0 if unspecified.
    pub damage_reach: f32,
}

/// Retail multiplayer weapon-list entries with a preferred off-hand pairing.
/// Both definitions must also pass the engine's dual-wield flag test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DualGun {
    Smg,
    PlasmaRifle,
    PlasmaPistol,
    Needler,
}

impl DualGun {
    fn read(m: &dyn GuestMemory, weap: u32, datum: DatumHandle) -> R<Option<Self>> {
        if m.u32(weap + 0x12c)? & (3 << 22) == 0 {
            return Ok(None);
        }
        let matg = m.u32(guest_layout::points::GLOBALS_TAG.base)?;
        if matg == 0 {
            return Ok(None);
        }
        let Some(h) = DatumHandle::from_raw(m.u32(matg + 0x16c)?) else {
            return Ok(None);
        };
        let Some(tag) = Tags.entry(m, h)? else {
            return Ok(None);
        };
        if tag.group(m)? != GroupTag::from_chars(*b"mulg") {
            return Ok(None);
        }
        let meta = tag.address(m)?;
        if meta == 0 || m.i32(meta + 8)? <= 0 {
            return Ok(None);
        }
        let universal = m.u32(meta + 0xc)?;
        if universal == 0 {
            return Ok(None);
        }
        let n = m.i32(universal + 0x68)?;
        let list = m.u32(universal + 0x6c)?;
        if list != 0 {
            for (i, kind) in [
                (1, Self::Smg),
                (6, Self::PlasmaPistol),
                (7, Self::PlasmaRifle),
                (8, Self::PlasmaRifle),
                (9, Self::Needler),
            ] {
                if (i as i32) < n && m.u32(list + 8 * i + 4)? == datum.raw() {
                    return Ok(Some(kind));
                }
            }
        }
        Ok(None)
    }

    fn pairs(self, other: Self) -> bool {
        use DualGun::*;
        matches!(
            (self, other),
            (Smg, Smg | PlasmaRifle | PlasmaPistol)
                | (PlasmaRifle | PlasmaPistol, Smg)
                | (Needler, Needler)
        )
    }
}

/// What a weapon is for, from its [`Profile`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Role {
    /// Not known: the old, weapon-blind behaviour.
    #[default]
    Unknown,
    /// No trigger, but aim assist: the energy sword. Close in and swing.
    Blade,
    /// Many pellets, once per pull: the shotgun.
    Scatter,
    /// Fires while held: SMG, plasma rifle, needler, and the plasma
    /// pistol's quick shots.
    Spray,
    /// Once per pull, accurate: magnum, battle rifle, carbine.
    Precise,
    /// Two zoom levels: the sniper and beam rifles.
    Sniper,
    /// Once per pull, a slow projectile: rocket launcher, fuel rod, brute
    /// shot. Splash: never close.
    Blast,
}

/// Slower than this (world units a second) a single-shot projectile is a
/// rocket, a fuel rod or a brute shot grenade, not a bullet.
pub const BLAST_SPEED: f32 = 100.0;
/// This many projectiles a shot or more is a shotgun.
pub const SCATTER_PELLETS: i16 = 4;

impl Profile {
    pub fn role(&self) -> Role {
        match self.trigger {
            Trigger::Unknown => Role::Unknown,
            Trigger::Absent => {
                // The retail sword has aim assist and no trigger.
                if self.cone_range > 0.0 || self.pull_range > 0.0 {
                    Role::Blade
                } else {
                    Role::Unknown
                }
            }
            t => {
                let single = t == Trigger::Single;
                if self.zoom >= 2 {
                    Role::Sniper
                } else if single && self.shot_speed > 0.0 && self.shot_speed < BLAST_SPEED {
                    Role::Blast
                } else if self.pellets >= SCATTER_PELLETS {
                    Role::Scatter
                } else if single {
                    Role::Precise
                } else {
                    Role::Spray
                }
            }
        }
    }
}

impl Role {
    /// The distances (world units, about 3 m each) it is best at: the
    /// movement keeps its target in here, and the weapon choice prefers
    /// the gun whose band holds the target.
    pub fn band(self) -> (f32, f32) {
        match self {
            Role::Unknown => (2.5, 15.0),
            Role::Blade => (0.0, 1.5),
            Role::Scatter => (0.0, 4.0),
            Role::Spray => (1.5, 9.0),
            Role::Precise => (5.0, 28.0),
            Role::Sniper => (12.0, 60.0),
            Role::Blast => (5.0, 25.0),
        }
    }
    /// Shoots at targets from `.0` to `.1` away: to
    /// [`crate::weapons::FIRE_PAST_BAND`] times its band's far end, the
    /// rule every gun fires by (wasted rounds past it) and, with splash,
    /// no closer than its own blast. A blade does not shoot.
    pub fn fire_range(self) -> (f32, f32) {
        let near = match self {
            Role::Blade => return (0.0, 0.0),
            Role::Blast => 2.5,
            _ => 0.0,
        };
        (near, self.band().1 * crate::weapons::FIRE_PAST_BAND)
    }
    /// How much it is worth going for (power weapons 4 and up).
    pub fn rating(self) -> u8 {
        match self {
            Role::Unknown => 1,
            Role::Spray => 2,
            Role::Precise => 3,
            Role::Scatter | Role::Blade => 4,
            Role::Sniper | Role::Blast => 5,
        }
    }
    pub fn power(self) -> bool {
        self.rating() >= 4
    }
    /// How far `d` is outside the band, relative to it: 0 inside.
    pub fn misfit(self, d: f32) -> f32 {
        let (lo, hi) = self.band();
        if d < lo {
            (lo - d) / lo.max(1.0)
        } else if d > hi {
            (d - hi) / hi.max(1.0)
        } else {
            0.0
        }
    }
}

/// One weapon: its definition (the `weap` tag datum), its ammunition and
/// its handling.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Gun {
    pub def: u32,
    pub ammo: Ammo,
    pub profile: Profile,
}

impl Gun {
    pub fn role(&self) -> Role {
        self.profile.role()
    }
    /// The distances it is good at, from its role and its tag's aim assist
    /// reach ([`crate::weapons::reach`]).
    pub fn reach(&self) -> crate::weapons::Reach {
        crate::weapons::reach(&self.profile)
    }
    /// How far `d` is outside its band, relative to it: 0 inside.
    pub fn misfit(&self, d: f32) -> f32 {
        crate::weapons::misfit(self.reach().band, d)
    }
    fn can_attack_at(&self, d: f32) -> bool {
        let reach = self.reach();
        if self.role() == Role::Blade {
            d < reach.lunge
        } else {
            (reach.fire.0..reach.fire.1).contains(&d)
        }
    }
    /// Worth going for: its role's rating, nothing when dry. A blade with
    /// no battery read counts as live.
    pub fn rating(&self) -> u8 {
        let role = self.role();
        if self.ammo.dry() && !(role == Role::Blade && self.ammo == Ammo::None) {
            0
        } else {
            role.rating()
        }
    }
    /// Able to fight now: a blade, or a gun with something loaded.
    pub fn ready(&self) -> bool {
        (self.role() == Role::Blade && !matches!(self.ammo, Ammo::Battery(b) if b <= BATTERY_DEAD))
            || self.ammo.ready()
    }
}

/// Whatever has a preferred range: a weapon, a role, what is carried, or
/// the weapons decision ([`preferred_range`]).
pub trait Ranged {
    fn band(&self) -> (f32, f32);
}

impl Ranged for Role {
    fn band(&self) -> (f32, f32) {
        Role::band(*self)
    }
}
impl Ranged for Profile {
    fn band(&self) -> (f32, f32) {
        self.role().band()
    }
}
impl Ranged for Gun {
    fn band(&self) -> (f32, f32) {
        self.reach().band
    }
}
impl Ranged for Arms {
    /// The held weapon's (an empty magazine is soon reloaded); the held
    /// one dry, the other's (it switches); nothing to fire, arm's length.
    fn band(&self) -> (f32, f32) {
        let live = |g: &Gun| g.ready() || !g.ammo.dry();
        match (self.held, self.other) {
            (Some(g), _) if live(&g) => g.band(),
            (_, Some(g)) if live(&g) => g.band(),
            _ => MELEE_BAND,
        }
    }
}
impl Ranged for Kit {
    /// The decision alone knows only melee: arm's length; anything else
    /// the weapon-blind band. Prefer [`Arms`] (or `Mind::fight.band`).
    fn band(&self) -> (f32, f32) {
        match self {
            Kit::Melee => MELEE_BAND,
            _ => Role::Unknown.band(),
        }
    }
}

/// With nothing to shoot: run in to melee.
pub const MELEE_BAND: (f32, f32) = (0.0, 1.0);

/// The distances (min, max; world units) the bot wants its target at, for
/// the movement: what it carries ([`Arms`], the held weapon), a [`Gun`],
/// a [`Role`], or the weapons decision ([`Kit`]: melee or not).
pub fn preferred_range(kit: &impl Ranged) -> (f32, f32) {
    kit.band()
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
    /// The grenade type selected ([`UNIT_GRENADE_SELECTED`]): 0 frag, 1
    /// plasma.
    pub selected: i8,
    /// The left-hand gun when dual wielding ([`UNIT_LEFT_WEAPON_INDEX`]):
    /// the left trigger fires it, and no grenade can be thrown.
    pub left: Option<Gun>,
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
    /// Hold Y in pickup range to fill the left hand.
    Dual(Pickup),
    /// Nothing to shoot: close in and melee.
    Melee,
}

/// The brain rolls once per compatible (primary definition, ground object)
/// during a life. A vanished pickup can reappear in view with the same salted
/// datum; only a fresh weapon instance or a different primary gets a new roll.
#[derive(Clone, Debug)]
pub(crate) struct DualChoices {
    rng: crate::mind::feel::Rng,
    choices: rustc_hash::FxHashMap<(u32, u32), bool>,
}

impl DualChoices {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            rng: crate::mind::feel::Rng::new(seed ^ 0x7a46_911d_b34c_205f),
            choices: Default::default(),
        }
    }

    pub(crate) fn new_life(&mut self) {
        self.choices.clear();
    }

    fn accepts(&mut self, primary: u32, pickup: u32, chance: f32) -> bool {
        *self.choices.entry((primary, pickup))
            .or_insert_with(|| self.rng.unit() < chance)
    }
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
    /// No enemy in view for a moment (it went behind cover): time for a
    /// reload of a half-empty magazine.
    pub breather: bool,
    /// More sensitive range selection and advanced inventory decisions,
    /// such as switching instead of reloading under fire (Normal and up).
    pub by_range: bool,
    pub pickups: &'a [Pickup],
}

/// Goes for weapons on the ground within this (world units).
pub const SEEK_RANGE: f32 = 30.0;
/// ...or, merely low and calm, within this.
pub const TOP_UP_RANGE: f32 = 12.0;
/// ...or, for a power weapon it does not have, out of a fight, within this.
pub const POWER_RANGE: f32 = 25.0;
/// Rather than walk away, goes for the enemy with no ammunition this close.
pub const MELEE_CHASE: f32 = 2.5;
/// In a lull, reloads a magazine under this.
pub const BREATHER_RELOAD: f32 = 0.6;
/// Calm with no enemy in view, tops up a magazine under this: the tactical
/// reload after a fight.
pub const TACTICAL_RELOAD: f32 = 0.9;
/// Required improvement in range fit for switching between two usable
/// weapons on Normal and up. This keeps small range changes stable.
pub const RANGE_SWITCH_MISFIT: f32 = 0.3;
/// A better pickup counts this many world units nearer per rating point.
pub const RATING_PULL: f32 = 3.0;

fn dist(a: Vec3, b: Vec3) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// The decision, pure. In order:
///
/// 1. the held weapon is a blade: keep it close up; with the enemy out of
///    its reach and a ready gun, switch (by range);
/// 2. the held gun is dry: switch to the other if it has ammunition; else
///    melee an enemy within [`MELEE_CHASE`]; else go for a pickup within
///    [`SEEK_RANGE`] (the best near one); else melee whatever comes;
/// 3. an empty magazine with rounds to load: reload; by range, with the
///    other gun loaded and the enemy in its band, switch instead (faster);
/// 4. prefer a ready sword or shotgun at close range, a sniper at the
///    far end of a rifle's band, or a gun that can reach the enemy;
/// 5. calm with a magazine under [`RELOAD_BELOW`], or under
///    [`TACTICAL_RELOAD`] with no enemy in view, or a lull with one under
///    [`BREATHER_RELOAD`], or the enemy beyond the gun's fire range with
///    one under [`RELOAD_BELOW`]: reload;
/// 6. no enemy in view and no power weapon carried: a power weapon within
///    [`POWER_RANGE`]: seek it, switching first so the worse gun is the one
///    dropped for it;
/// 7. calm with both weapons low: top up from a pickup within
///    [`TOP_UP_RANGE`];
/// 8. otherwise fine.
///
/// `skip` rules out pickups (those the bot could not reach).
pub fn plan(s: &Situation<'_>, skip: impl Fn(&Pickup) -> bool) -> Kit {
    plan_with_personality(s, skip, None, None)
}

pub(crate) fn plan_with_personality(
    s: &Situation<'_>,
    skip: impl Fn(&Pickup) -> bool,
    personality: Option<crate::decisions::Personality>,
    mut dual: Option<(&mut DualChoices, f32)>,
) -> Kit {
    let held_gun = s.arms.held;
    let other_gun = s.arms.other;
    let held = held_gun.map_or(Ammo::None, |g| g.ammo);
    let role = held_gun.map_or(Role::Unknown, |g| g.role());
    let other = other_gun.map(|g| g.ammo);
    let other_ready = other_gun.is_some_and(|g| g.ready());
    let other_role = other_gun.map_or(Role::Unknown, |g| g.role());
    // Each gun's band and fire range: its role's, refined by its aim
    // assist reach (crate::weapons::reach).
    let held_misfit = |d: f32| held_gun.map_or(Role::Unknown.misfit(d), |g| g.misfit(d));
    let other_misfit = |d: f32| other_gun.map_or(Role::Unknown.misfit(d), |g| g.misfit(d));
    // All pickup choices, including the off hand, apply interest after
    // filtering for reach, safety and weapon compatibility.
    let nearest = |range: f32, want: &mut dyn FnMut(&Pickup) -> bool| {
        s.pickups
            .iter()
            .map(|p| (dist(p.position, s.at), *p))
            .filter(|(d, p)| *d < range && p.useful() && !skip(p) && want(p))
            .min_by(|a, b| {
                let cost = |(d, p): &(f32, Pickup)| {
                    d - RATING_PULL * p.gun.rating() as f32
                        - crate::weapons::pickup_bias(personality, p.gun.role())
                };
                cost(a).total_cmp(&cost(b))
            })
            .map(|(_, p)| p)
    };
    // Draw a ready close-range power weapon even when the current gun
    // also fits. Otherwise keep the current gun until the mismatch is clear.
    let range_switch = |d: f32| {
        let Some(other) = other_gun.filter(|g| g.ready() && g.role() != Role::Unknown) else {
            return false;
        };
        let other_fit = other.misfit(d);
        let close_power = matches!(other_role, Role::Blade | Role::Scatter)
            && other_fit == 0.0
            && role != Role::Blade
            && (role != Role::Scatter || other_role == Role::Blade);
        let sniper_distance = other_role == Role::Sniper
            && role == Role::Precise
            && other_fit == 0.0
            && held_gun.is_some_and(|g| d > 0.8 * g.reach().band.1);
        let threshold = if s.by_range { RANGE_SWITCH_MISFIT } else { 0.6 };
        close_power
            || sniper_distance
            || (other.can_attack_at(d)
                && (held_gun.is_some_and(|g| !g.can_attack_at(d))
                    || held_misfit(d) - other_fit > threshold))
    };
    if role == Role::Blade && held_gun.is_some_and(|g| g.ready()) {
        if let Some(d) = s.enemy
            && range_switch(d)
        {
            return Kit::Switch;
        }
        return Kit::Fine;
    }
    if held.dry() {
        if other.is_some_and(|o| !o.dry()) || other_ready {
            return Kit::Switch;
        }
        if s.enemy.is_some_and(|d| d < MELEE_CHASE) {
            return Kit::Melee;
        }
        if let Some(p) = nearest(SEEK_RANGE, &mut |_| true) {
            return Kit::Seek(p);
        }
        return if s.enemy.is_some() {
            Kit::Melee
        } else {
            Kit::Fine
        };
    }
    if held.empty_magazine() {
        if let Some(d) = s.enemy
            && s.by_range
            && other_ready
            && other_misfit(d) == 0.0
        {
            return Kit::Switch;
        }
        return Kit::Reload;
    }
    if let Some(d) = s.enemy
        && range_switch(d)
    {
        return Kit::Switch;
    }
    let fire_to = held_gun.map_or(role.fire_range().1, |g| g.reach().fire.1);
    let out_of_reach = s.enemy.is_some_and(|d| d > fire_to);
    let loadable = matches!(held, Ammo::Rounds { reserve, .. } if reserve > 0);
    let half = loadable && held.magazine() < BREATHER_RELOAD;
    let topped = loadable && held.magazine() < TACTICAL_RELOAD;
    if (held.wants_reload() && (s.calm || out_of_reach))
        || (s.breather && half)
        || (s.calm && s.enemy.is_none() && topped)
    {
        return Kit::Reload;
    }
    let has_power = [held_gun, other_gun]
        .iter()
        .flatten()
        .any(|g| g.rating() > 0 && g.role().power());
    let power_range = if s.enemy.is_some() {
        crate::mind::TAKE_RANGE
    } else {
        POWER_RANGE
    };
    if (s.enemy.is_some() || s.calm || s.breather)
        && !has_power
        && let Some(p) = nearest(power_range, &mut |p| {
            p.gun.role().power() && !s.arms.carries(p.gun.def)
        })
    {
        // Taking it drops the held gun: make that the worse one.
        let rate = |g: Option<Gun>| g.map_or(0, |g| g.rating());
        if other_gun.is_some() && rate(held_gun) > rate(other_gun) {
            return Kit::Switch;
        }
        return Kit::Seek(p);
    }
    if (dual.is_some() || s.by_range)
        && s.arms.left.is_none()
        && let Some(h) = held_gun.filter(|h| h.ready())
        && let Some(kind) = h.profile.dual
        && let Some(p) = nearest(
            if s.enemy.is_some() {
                crate::mind::TAKE_RANGE
            } else {
                PAIR_RANGE
            },
            &mut |p| {
                p.gun.ready()
                    && p.gun.profile.dual.is_some_and(|other| kind.pairs(other))
                    && s.enemy
                        .is_none_or(|d| d <= h.reach().band.1.min(p.gun.reach().band.1))
                    && dual.as_mut().is_none_or(|(choices, chance)| {
                        choices.accepts(h.def, p.id, *chance)
                    })
            },
        )
    {
        return Kit::Dual(p);
    }
    if s.calm && held.low() && other.is_none_or(|o| o.low()) {
        // Same kind tops up by walking over it; any useful gun will do.
        if let Some(p) = nearest(TOP_UP_RANGE, &mut |_| true) {
            return Kit::Seek(p);
        }
    }
    if s.calm
        && s.enemy.is_none()
        && let Some(traits) = personality
        && ![held_gun, other_gun]
            .into_iter()
            .flatten()
            .any(|g| g.rating() > 0 && g.role() == traits.favorite_weapon)
        && let Some(p) = nearest(traits.favorite_detour(), &mut |p| {
            traits.weapon_interest(p.gun.role()) > 0.0 && !s.arms.carries(p.gun.def)
        })
    {
        let rate = |g: Option<Gun>| g.map_or(0, |g| g.rating());
        // Preserve the stronger gun and never trade a stronger pair just
        // for a favorite. A single gun has an open second slot.
        if other_gun.is_none() || p.gun.rating() >= rate(held_gun).min(rate(other_gun)) {
            return if other_gun.is_some() && rate(held_gun) > rate(other_gun) {
                Kit::Switch
            } else {
                Kit::Seek(p)
            };
        }
    }
    // A single short gun leaves the second slot free for a rifle, even
    // with full ammunition. Walking onto it and holding X is retail input.
    if s.calm && other_gun.is_none()
        && let Some(h) = held_gun.filter(|g| g.role() != Role::Unknown)
        && let Some(p) = nearest(PAIR_RANGE, &mut |p| {
            !s.arms.carries(p.gun.def) && p.gun.rating() > h.rating()
        })
    {
        return Kit::Seek(p);
    }
    if s.calm && s.by_range {
        // A long gun and a short one: two of a kind, it takes one of the
        // other kind, dropping the worse of the two.
        if let (Some(h), Some(o)) = (held_gun, other_gun) {
            let (ch, co) = (crate::weapons::class(&h), crate::weapons::class(&o));
            if ch == co
                && ch != crate::weapons::Class::Other
                && let Some(p) = nearest(PAIR_RANGE, &mut |p| {
                    let c = crate::weapons::class(&p.gun);
                    c != ch && c != crate::weapons::Class::Other && !s.arms.carries(p.gun.def)
                })
            {
                if h.rating() > o.rating() {
                    return Kit::Switch;
                }
                return Kit::Seek(p);
            }
        }
        // The other gun's magazine wants a reload and this one is full:
        // draw it now, so the reload happens out of the fight.
        if held.magazine() >= FULL_ENOUGH && other_gun.is_some_and(|o| o.ammo.wants_reload()) {
            return Kit::Switch;
        }
    }
    Kit::Fine
}

/// Out of a fight, a gun that pairs a short one with a long one is taken
/// within this (world units).
pub const PAIR_RANGE: f32 = 15.0;
/// A magazine this full needs nothing: the other gun is reloaded first.
pub const FULL_ENOUGH: f32 = 0.9;

// ---- reading it from the guest ----

/// A `weap` definition's ammunition layout, read from the tag.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct WeaponDef {
    /// The first magazine's rounds loaded and reserve maxima, if it has
    /// magazines.
    pub magazine: Option<(i16, i16)>,
    /// No magazines or a barrel that uses battery per shot.
    pub ages: bool,
}

impl WeaponDef {
    /// Reads the definition at `weap` (the tag's metadata).
    pub fn read(m: &dyn GuestMemory, weap: u32) -> R<WeaponDef> {
        if m.u16(weap + 0x290)? != 0 {
            return Ok(WeaponDef::default());
        }
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
        // 0x101d20 accepts zero magazines immediately, including the sword.
        // Objective weapons also have zero magazines, but cannot be supplies.
        let mut ages = mags == 0;
        for i in 0..barrels {
            if m.f32(base + i * BARREL_DEF_STRIDE + BARREL_DEF_USAGE)? > 0.0 {
                ages = true;
            }
        }
        Ok(WeaponDef { magazine, ages })
    }

    /// The handling of the weapon whose definition is at `weap` (section
    /// 3.6, "Handling"). The first trigger and the first barrel speak for
    /// the weapon. A projectile that does not resolve supplies neither
    /// speed nor damage reach.
    pub fn profile(m: &dyn GuestMemory, weap: u32) -> R<Profile> {
        let triggers = m.i32(weap + WEAP_TRIGGERS)?;
        let trigger = if triggers <= 0 {
            Trigger::Absent
        } else {
            let first = m.u32(weap + WEAP_TRIGGERS + 4)?;
            Trigger::from_behaviour(m.u16(first + TRIGGER_DEF_BEHAVIOUR)?)
        };
        let mut p = Profile {
            trigger,
            zoom: m.u16(weap + WEAP_ZOOM_LEVELS)? as i16,
            cone_range: finite(m.f32(weap + WEAP_CONE_RANGE)?),
            pull_range: finite(m.f32(weap + WEAP_PULL_RANGE)?),
            ..Profile::default()
        };
        if m.i32(weap + WEAP_BARRELS)? > 0 {
            let barrel = m.u32(weap + WEAP_BARRELS + 4)?;
            p.rate = finite(m.f32(barrel + BARREL_DEF_RATE)?);
            p.pellets = m.u16(barrel + BARREL_DEF_PROJECTILES)? as i16;
            let proj = m.u32(barrel + BARREL_DEF_PROJECTILE + 4)?;
            if let Some(h) = DatumHandle::from_raw(proj)
                && let Some(tag) = Tags.entry(m, h)?
                && tag.group(m)? == PROJ
            {
                let def = tag.address(m)?;
                if def != 0 {
                    p.shot_speed = finite(m.f32(def + PROJ_INITIAL_VELOCITY)?);
                    let damage = finite(m.f32(def + PROJ_AIR_DAMAGE_RANGE_END)?);
                    let travel = finite(m.f32(def + PROJ_MAXIMUM_RANGE)?);
                    p.damage_reach = match (damage > 0.0, travel > 0.0) {
                        (true, true) => damage.min(travel),
                        _ => damage.max(travel),
                    };
                }
            }
        }
        Ok(p)
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
    // The handling is a refinement: a fault reading it leaves the gun
    // weapon-blind, not unread.
    let mut profile = WeaponDef::profile(m, weap).unwrap_or_default();
    profile.dual = DualGun::read(m, weap, def).ok().flatten();
    Ok(Some((
        Gun {
            def: def.raw(),
            ammo,
            profile,
        },
        body,
    )))
}

/// `v` when finite and not negative, else 0 (unknown).
fn finite(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

/// What the unit whose body is at `unit` carries.
pub fn read(m: &dyn GuestMemory, unit: u32) -> R<Arms> {
    let held_slot = m.u8(unit + UNIT_WEAPON_INDEX)? as i8;
    let left_slot = m.u8(unit + UNIT_LEFT_WEAPON_INDEX)? as i8;
    let mut arms = Arms {
        grenades: [
            m.u8(unit + UNIT_GRENADES)? as i8,
            m.u8(unit + UNIT_GRENADES + 1)? as i8,
        ],
        selected: m.u8(unit + UNIT_GRENADE_SELECTED)? as i8,
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
        } else if slot as i32 == i32::from(left_slot) {
            arms.left = Some(g);
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
    Ok(ground(m, None)?.weapons)
}

pub(crate) struct Ground {
    pub loose_grenades: Vec<(crate::brain::Vec3, crate::grenades::Kind)>,
    pub held_power: Vec<u32>,
    pub weapons: Vec<Pickup>,
    pub supplies: Option<Vec<crate::decisions::supplies::Supply>>,
    /// Live grenades within [`crate::incoming::DANGER`] of `near`: none
    /// when no `near` was given.
    pub grenades: Vec<crate::incoming::Grenade>,
}

/// Scan the object table once for both ordinary pickups and occupied power
/// placements. A bad equipment tag disables timing for the sample, while
/// the independently readable weapons still reach the ordinary arms plan.
/// With `near` (the bot's feet), the same scan looks for live grenades
/// close to it (crate::incoming).
pub(crate) fn ground(m: &dyn GuestMemory, near: Option<crate::brain::Vec3>) -> R<Ground> {
    let mut out = Ground {
        loose_grenades: Vec::new(),
        held_power: Vec::new(),
        weapons: Vec::new(),
        supplies: Some(Vec::new()),
        grenades: Vec::new(),
    };
    ground_into(m, near, &mut out, None)?;
    Ok(out)
}

pub(crate) fn ground_into(
    m: &dyn GuestMemory,
    near: Option<crate::brain::Vec3>,
    out: &mut Ground,
    vehicle_headers: Option<&mut Vec<crate::vehicle::read::VehicleHeader>>,
) -> R<()> {
    scan_into(
        m,
        near.map_or(Senses::Blind, Senses::Near),
        out,
        vehicle_headers,
    )
}

/// Which live grenades a scan keeps.
#[derive(Clone, Copy)]
enum Senses {
    Blind,
    /// Those within [`crate::incoming::DANGER`] of this point.
    Near(crate::brain::Vec3),
    /// Every one, for a [`SharedGround`] to filter per bot.
    All,
}

/// A live grenade at `position` is within [`crate::incoming::DANGER`] of `at`.
fn in_danger(position: crate::brain::Vec3, at: crate::brain::Vec3) -> bool {
    let distance = (position[0] - at[0])
        .hypot(position[1] - at[1])
        .hypot(position[2] - at[2]);
    distance < crate::incoming::DANGER
}

fn scan_into(
    m: &dyn GuestMemory,
    senses: Senses,
    out: &mut Ground,
    mut vehicle_headers: Option<&mut Vec<crate::vehicle::read::VehicleHeader>>,
) -> R<()> {
    use crate::incoming::{Grenade, PROJECTILE, grenade_defs};

    out.held_power.clear();
    out.weapons.clear();
    out.grenades.clear();
    out.loose_grenades.clear();
    let Some(array) = objects::array(m)? else {
        out.supplies = None;
        return Ok(());
    };
    out.supplies.get_or_insert_with(Vec::new).clear();
    let grenade_equipment = crate::incoming::equipment_defs(m);
    let senses = match senses {
        Senses::Blind => None,
        Senses::Near(at) => grenade_defs(m).map(|defs| (Some(at), defs)),
        Senses::All => grenade_defs(m).map(|defs| (None, defs)),
    };
    for record in array.live_iter(m, OBJECT_LIMIT)? {
        let (h, header) = record?;
        let site = match header.kind(m)? {
            objects::VEHICLE => {
                if let Some(headers) = &mut vehicle_headers {
                    headers.push((h, header));
                }
                continue;
            }
            PROJECTILE => {
                if let Some((at, defs)) = senses
                    && let Ok(addr @ 1..) = header.body(m)
                {
                    let body = objects::Body::at(addr);
                    if let Ok(Some(def)) = body.definition(m)
                        && defs.contains(&Some(def))
                        && let Ok(position) = body.position(m)
                        && at.is_none_or(|at| in_danger(position, at))
                    {
                        out.grenades.push(Grenade {
                            id: h.raw(),
                            at: position,
                        });
                    }
                }
                continue;
            }
            objects::WEAPON => {
                let Some((gun, body)) = gun(m, h)? else {
                    continue;
                };
                if body.parent(m)?.is_some() {
                    if gun.role().power() {
                        out.held_power.push(h.raw());
                    }
                    continue;
                }
                out.weapons.push(Pickup {
                    id: h.raw(),
                    position: body.position(m)?,
                    gun,
                });
                if out.supplies.is_none() || !gun.role().power() {
                    continue;
                }
                crate::guest::intel::supply(m, h, body, None, gun.role())
            }
            crate::guest::intel::EQUIPMENT => header.body(m).and_then(|addr| {
                if addr != 0 && let Some(defs) = grenade_equipment {
                    let body = objects::Body::at(addr);
                    if let Some(def) = body.definition(m)?
                        && let Some(i) = defs.iter().position(|&d| d == Some(def))
                    {
                        if body.parent(m)?.is_none()
                            && let Some(kind) = crate::grenades::Kind::from_index(i as i8)
                        {
                            out.loose_grenades.push((body.position(m)?, kind));
                        }
                        return Ok(None);
                    }
                }
                if out.supplies.is_some() {
                    crate::guest::intel::equipment(m, h, addr)
                } else {
                    Ok(None)
                }
            }),
            _ => continue,
        };
        match site {
            Ok(Some(site)) => {
                if let Some(sites) = &mut out.supplies {
                    sites.push(site);
                }
            }
            Ok(None) => {}
            Err(_) => out.supplies = None,
        }
    }
    Ok(())
}

/// One object-table scan shared by every bot sampled for one game tick.
///
/// What a scan finds (pickups, supplies, held power, loose and live
/// grenades, vehicles) is the same for every bot but for the live
/// grenades' distance, so a sampler that runs several bots against one
/// guest state opens a scope ([`share_ground`]) and the table is walked
/// once, not once per bot. The bots of a tick see the memory of its first
/// sample, as a batched sample does (`think::Mode::Batched`).
#[derive(Default)]
pub(crate) struct SharedGround {
    key: Option<u64>,
    scan: Option<Scan>,
    /// Table walks made, for tests.
    scans: u32,
}

struct Scan {
    result: R<()>,
    ground: Ground,
    vehicles: Vec<crate::vehicle::read::VehicleHeader>,
}

impl SharedGround {
    #[cfg(test)]
    pub(crate) fn scans(&self) -> u32 {
        self.scans
    }
}

thread_local! {
    static SHARED: std::cell::RefCell<Option<SharedGround>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` with `store` as this thread's shared scan. A store keyed
/// otherwise is emptied first; an open scope already on the thread is
/// kept (the outer sampler's store holds the scan).
pub(crate) fn share_ground<T>(store: &mut SharedGround, key: u64, f: impl FnOnce() -> T) -> T {
    if SHARED.with_borrow(Option::is_some) {
        return f();
    }
    if store.key != Some(key) {
        store.key = Some(key);
        store.scan = None;
    }
    struct Restore<'a>(&'a mut SharedGround);
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            if let Some(s) = SHARED.take() {
                *self.0 = s;
            }
        }
    }
    SHARED.set(Some(std::mem::take(store)));
    let _restore = Restore(store);
    f()
}

/// [`ground_into`] through the thread's shared scan when a scope is open
/// ([`share_ground`]): the table is walked at the first bot's call and
/// copied out for the others, live grenades filtered by each one's `near`.
pub(crate) fn ground_for(
    m: &dyn GuestMemory,
    near: Option<crate::brain::Vec3>,
    out: &mut Ground,
    vehicle_headers: Option<&mut Vec<crate::vehicle::read::VehicleHeader>>,
) -> R<()> {
    if !SHARED.with_borrow(Option::is_some) {
        return ground_into(m, near, out, vehicle_headers);
    }
    SHARED.with_borrow_mut(|shared| {
        let shared = shared.as_mut().expect("the scope checked above");
        if shared.scan.is_none() {
            shared.scans += 1;
            let mut ground = Ground {
                loose_grenades: Vec::new(),
                held_power: Vec::new(),
                weapons: Vec::new(),
                supplies: Some(Vec::new()),
                grenades: Vec::new(),
            };
            let mut vehicles = Vec::new();
            let result = scan_into(m, Senses::All, &mut ground, Some(&mut vehicles));
            shared.scan = Some(Scan {
                result,
                ground,
                vehicles,
            });
        }
        let scan = shared.scan.as_ref().expect("scanned above");
        // A fault leaves what the walk found before it, as ground_into does.
        if let Some(headers) = vehicle_headers {
            headers.extend_from_slice(&scan.vehicles);
        }
        scan.result?;
        let g = &scan.ground;
        out.loose_grenades.clear();
        out.loose_grenades.extend_from_slice(&g.loose_grenades);
        out.held_power.clear();
        out.held_power.extend_from_slice(&g.held_power);
        out.weapons.clear();
        out.weapons.extend_from_slice(&g.weapons);
        match (&g.supplies, &mut out.supplies) {
            (None, s) => *s = None,
            (Some(from), Some(to)) => {
                to.clear();
                to.extend_from_slice(from);
            }
            (Some(from), s @ None) => *s = Some(from.clone()),
        }
        out.grenades.clear();
        if let Some(at) = near {
            out.grenades
                .extend(g.grenades.iter().filter(|n| in_danger(n.at, at)).copied());
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests;
