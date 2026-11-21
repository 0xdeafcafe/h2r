//! Units, bipeds, weapons and devices: the type-specific parts of an
//! object's body (docs/live/live-gamestate.md "Unit aim, held weapon, species and shots").
//!
//! Offsets are from the body's start, as [`Object`]'s are. `unit_new`
//! (`0x000c4410`) is the evidence for most unit fields: it sets the four
//! weapon datums at `+0x218` and the two current-weapon indices at
//! `+0x212`/`+0x213` to none, and initialises five vectors from the object
//! forward vector (`+0x150`, `+0x15c`, `+0x168`, `+0x180`, `+0x18c`).

use crate::field::Datum;
use crate::mem::{Fault, Mem};
use crate::object::Object;

/// A unit holds four weapons.
pub const WEAPON_SLOTS: usize = 4;

crate::layout! {
    /// A unit's body (bipeds and vehicles), past the shared object fields.
    pub struct Unit: 0x2c8 {
        /// The unit's actor datum, or none: what makes it AI-controlled.
        /// Confirmed: `0x000c6810` indexes the actor array
        /// ([`crate::global::ACTORS`], stride `0x888`) with it.
        ACTOR actor @ 0x12c: Datum,
        /// Unit flags. Bit 23 ([`CROUCH_REQUEST`]) is the crouch request.
        FLAGS flags @ 0x134: u32,
        /// The unit's team. Confirmed: `unit_new` takes it from the
        /// placement; `0x0014bfc0` copies the player's, `0x000c6810` the
        /// actor's.
        TEAM team @ 0x138: i16,
        /// The unit's player datum, or none (confirmed).
        PLAYER player @ 0x13c: Datum,
        /// The desired facing vector; the teleport writes it (`0x2a01a2`).
        DESIRED_FACING desired_facing @ 0x150: [f32; 3],
        /// The desired aiming vector, the one networked by `0x0009d450`;
        /// the teleport writes it (`0x2a01b8`).
        DESIRED_AIM desired_aim @ 0x15c: [f32; 3],
        /// likely: the aiming vector. `0x000cf3d0` casts the held weapon's
        /// targeting ray along it, so it is world space.
        AIM aim @ 0x168: [f32; 3],
        /// The desired looking vector; the teleport writes it (`0x2a01ce`).
        DESIRED_LOOK desired_look @ 0x180: [f32; 3],
        /// likely: the looking vector.
        LOOK look @ 0x18c: [f32; 3],
        /// Forward, left and up throttle from the unit control record
        /// (`0x000c6dfc`, filled for actors at `0x001e4403`).
        THROTTLE throttle @ 0x1b0: [f32; 3],
        /// The seat in the vehicle it rides (-1 none). Confirmed: the
        /// enter-vehicle routine (`0x000ea1f0`) stores it (`0x000ea405`).
        SEAT seat @ 0x1fc: i16,
        /// The held weapon's slot in [`Unit::WEAPONS`] (-1 none).
        WEAPON_INDEX weapon_index @ 0x212: i8,
        /// The left-hand weapon's slot when dual wielding (-1 none).
        LEFT_WEAPON_INDEX left_weapon_index @ 0x213: i8,
        /// The four weapon object datums. Readers `0x000c85c0` and
        /// `0x000cf3d0` index it by [`Unit::WEAPON_INDEX`].
        WEAPONS weapons @ 0x218: [Datum; 4],
        /// The selected grenade type (-1 none); likely 0 frag, 1 plasma.
        GRENADE_INDEX grenade_index @ 0x23c: i8,
        /// Grenades carried by type. Confirmed: `0x000cdff0` and the HUD
        /// (`0x002759cf`) index it with [`Unit::GRENADE_INDEX`].
        GRENADE_COUNTS grenade_counts @ 0x23e: [i8; 2],
        /// Crouch amount, 0 standing to 1 crouched. Confirmed: the crouch
        /// update (`0x000dd83f`) moves it toward 1 while [`CROUCH_REQUEST`]
        /// is set.
        CROUCH crouch @ 0x2c4: f32,
    }
}

/// [`Unit::FLAGS`] bit 23: crouch requested (`0x000e291b` sets it).
pub const CROUCH_REQUEST: u32 = 1 << 23;

impl Unit {
    /// The shared object fields of the same body.
    pub const fn object(self) -> Object {
        Object::at(self.addr)
    }
    /// The datum in weapon slot `slot`, `None` outside 0..4 or when empty.
    pub fn weapon<M: Mem + ?Sized>(&self, m: &M, slot: i8) -> Result<Option<Datum>, Fault> {
        if !(0..WEAPON_SLOTS as i8).contains(&slot) {
            return Ok(None);
        }
        let d = crate::Off::<Datum>::new(Unit::WEAPONS.offset + 4 * slot as u32).read(m, self.addr)?;
        Ok((!d.is_none()).then_some(d))
    }
    /// The held weapon's datum, if it holds one.
    pub fn held_weapon<M: Mem + ?Sized>(&self, m: &M) -> Result<Option<Datum>, Fault> {
        self.weapon(m, self.weapon_index(m)?)
    }
}

crate::layout! {
    /// A biped's body, past the unit fields.
    pub struct Biped: 0x3f8 {
        /// Biped flags. Bit 0 counts airborne ticks into
        /// [`Biped::AIRBORNE_TICKS`] (`0x000dd608`); likely: airborne.
        /// The high byte is [`SUPPORT_FLAGS`].
        FLAGS flags @ 0x348: u16,
        /// The ground support contact's point (contact `+0x1c`).
        SUPPORT_POINT support_point @ 0x368: [f32; 3],
        /// The ground support contact's normal (contact `+0x28`).
        SUPPORT_NORMAL support_normal @ 0x374: [f32; 3],
        /// The support contact's point, second copy (`0x000dde99..0x000ddef5`).
        SUPPORT_CONTACT_POINT support_contact_point @ 0x380: [f32; 3],
        /// Ticks bit 0 of [`Biped::FLAGS`] has been set (capped at 127).
        AIRBORNE_TICKS airborne_ticks @ 0x399: i8,
        /// Hitbox offset counter: a standing melee lunge sets it negative
        /// (`0x000e29b4`); it steps toward 0 each tick outside the lunge.
        HITBOX_OFFSET hitbox_offset @ 0x3aa: i8,
        /// likely: the physics mode, [`MODE_GROUND`] .. [`MODE_MELEE`].
        PHYSICS_MODE physics_mode @ 0x3dc: u8,
        /// The ground mode's state: the ground test (`0x001f19c0`) first
        /// takes the length of these three floats.
        GROUND_STATE ground_state @ 0x3ec: [f32; 3],
    }
}

/// The byte of [`Biped::FLAGS`]'s high half: bit 4 ([`SUPPORT_COPIED`]) is
/// set when the physics update copied a support contact (`0x000ddeda`).
pub const SUPPORT_FLAGS: u32 = 0x349;
pub const SUPPORT_COPIED: u8 = 0x10;
/// Five words naming what supports the biped (`0x000ddcd0..0x000ddce5`).
pub const SUPPORT_WORDS: u32 = 0x354;
pub const MODE_GROUND: u8 = 1;
pub const MODE_FLYING: u8 = 2;
pub const MODE_DEAD: u8 = 3;
pub const MODE_POSTURE: u8 = 4;
pub const MODE_CLIMBING: u8 = 5;
pub const MODE_MELEE: u8 = 6;

impl Biped {
    pub const fn unit(self) -> Unit {
        Unit::at(self.addr)
    }
    pub const fn object(self) -> Object {
        Object::at(self.addr)
    }
}

crate::layout! {
    /// A weapon's body, past the shared object fields.
    pub struct Weapon: 0x22e {
        /// The battery used, 0 to 1.
        AGE age @ 0x184: f32,
        /// The first magazine's reserve rounds (confirmed, `0x00101a10`).
        MAGAZINE_RESERVE magazine_reserve @ 0x22a: i16,
        /// The first magazine's loaded rounds (confirmed, `0x00101a10`).
        MAGAZINE_LOADED magazine_loaded @ 0x22c: i16,
    }
}

/// The magazine states start here, [`MAGAZINE_STRIDE`] apart; in each the
/// reserve is at `+2` and the loaded rounds at `+4`.
pub const MAGAZINES: u32 = 0x228;
pub const MAGAZINE_STRIDE: u32 = 0x10;

impl Weapon {
    pub const fn object(self) -> Object {
        Object::at(self.addr)
    }
}

crate::layout! {
    /// A device's body (machines, controls), past the shared object fields.
    pub struct Device: 0x144 {
        /// The normalized position. The updater (`0x0011546d`) adds the
        /// tick's travel; `0x00106930` hands it to the position channel.
        POSITION position @ 0x140: f32,
    }
}

/// The device's position animation channel (an embedded channel):
/// `0x00106930` uses it, `0x001086e0` starts the position action here.
pub const DEVICE_POSITION_CHANNEL: u32 = 0x18c;
