//! Objects: the header in the objects datum array and the body every type
//! shares (docs/formats/objects.md, docs/live/live-gamestate.md).
//!
//! A body's offsets are from the body's start for every type: a unit's
//! fields continue where the shared ones end, so [`Object::as_unit`] is the
//! same address viewed as a [`Unit`](crate::unit::Unit).

use crate::field::Datum;
use crate::mem::{Fault, Mem};
use crate::unit::{Biped, Unit, Weapon};

crate::layout! {
    /// An object header (`0xc` bytes), the element of the objects datum
    /// array ([`crate::global::OBJECTS`]).
    pub struct ObjectHeader: 0xc {
        /// Datum salt; zero when the slot is free.
        SALT salt @ 0x0: u16,
        /// likely: header flags.
        FLAGS flags @ 0x2: u8,
        /// The concrete type ([`ObjectType`]).
        KIND kind @ 0x3: u8,
        /// likely: the cluster the object is in.
        CLUSTER cluster @ 0x4: u16,
        /// The body's size: the allocator (`0x000bc2c6`) stores the type
        /// descriptor's runtime body size (`+8`, `0x000b7bd6`) here.
        BODY_SIZE body_size @ 0x6: u16,
        /// The object's body (its full state).
        BODY body @ 0x8: u32,
    }
}

impl ObjectHeader {
    /// The body, `None` while the header has none.
    pub fn object<M: Mem + ?Sized>(&self, m: &M) -> Result<Option<Object>, Fault> {
        let body = self.body(m)?;
        Ok((body != 0).then_some(Object::at(body)))
    }
    /// The type, `None` for a value outside the table.
    pub fn object_type<M: Mem + ?Sized>(&self, m: &M) -> Result<Option<ObjectType>, Fault> {
        Ok(ObjectType::from_u8(self.kind(m)?))
    }
}

crate::layout! {
    /// The start of an object's body: the part every type shares. The
    /// whole body is [`ObjectHeader::BODY_SIZE`] long.
    pub struct Object: 0x12c {
        /// The definition tag's full salted datum.
        DEFINITION definition @ 0x00: Datum,
        /// The object this one is attached to; its transform is then
        /// parent-relative.
        PARENT parent @ 0x14: Datum,
        /// The scenario placement index (`i16`, -1 for objects the scenario
        /// did not place). Confirmed: `object_new` copies it from the
        /// placement data's `+0x10` (`0x000b7c87`).
        PLACEMENT_INDEX placement_index @ 0x1a: i16,
        POSITION position @ 0x64: [f32; 3],
        FORWARD forward @ 0x70: [f32; 3],
        UP up @ 0x7c: [f32; 3],
        /// Translational velocity in world units a second, not a tick
        /// (confirmed on the projectile update, `0x000f8ee2` on).
        VELOCITY velocity @ 0x88: [f32; 3],
        /// The netgame equipment entry that spawned the object (`i8`, -1
        /// none). Confirmed: the spawner `0x0015aad0` writes it.
        NETGAME_EQUIPMENT netgame_equipment @ 0xaf: i8,
        /// The physics component index (-1 none) into
        /// [`crate::global::PHYSICS_COMPONENTS`] (confirmed, `0x001d08e0`).
        PHYSICS physics @ 0xb4: u32,
        /// Body vitality, 1.0 full. `object_get_health` (`0x002a1a20`)
        /// returns it, 0 instead while [`DEPLETED`] is set.
        HEALTH health @ 0xec: f32,
        /// Shield vitality, 1.0 full; an overshield charges past 1.
        /// `object_get_shield` (`0x002a1a90`).
        SHIELD shield @ 0xf0: f32,
        /// Damage flags: [`DEPLETED`], [`OVERSHIELD`].
        DAMAGE_FLAGS damage_flags @ 0x10a: u16,
        /// The node matrices' total size in bytes (`0x34` each).
        NODES_SIZE nodes_size @ 0x114: u16,
        /// The node matrices' offset from the body (signed; world space).
        NODES_OFFSET nodes_offset @ 0x116: i16,
        /// Where the animation manager is: an offset from the body, -1 none.
        /// Confirmed: `object_new` (`0x000b7b40`) builds it there.
        ANIMATION_MANAGER animation_manager @ 0x12a: i16,
    }
}

/// [`Object::DAMAGE_FLAGS`] bit 2: body depleted (`0x000d6800`).
pub const DEPLETED: u16 = 1 << 2;
/// [`Object::DAMAGE_FLAGS`] bit 4: an overshield is up.
pub const OVERSHIELD: u16 = 1 << 4;
/// One node matrix: 13 floats (scale, three axes, position).
pub const NODE_MATRIX_SIZE: u32 = 0x34;

impl Object {
    /// The same body as a unit's (bipeds and vehicles). Not checked: ask
    /// the header's type first.
    pub const fn as_unit(self) -> Unit {
        Unit::at(self.addr)
    }
    /// The same body as a biped's. Not checked.
    pub const fn as_biped(self) -> Biped {
        Biped::at(self.addr)
    }
    /// The same body as a weapon's. Not checked.
    pub const fn as_weapon(self) -> Weapon {
        Weapon::at(self.addr)
    }
    /// Whether the body is depleted ([`DEPLETED`]).
    pub fn depleted<M: Mem + ?Sized>(&self, m: &M) -> Result<bool, Fault> {
        Ok(self.damage_flags(m)? & DEPLETED != 0)
    }
    /// The node matrices: their address and count, `None` when the body
    /// has none.
    pub fn nodes<M: Mem + ?Sized>(&self, m: &M) -> Result<Option<(u32, u32)>, Fault> {
        let size = u32::from(self.nodes_size(m)?);
        let offset = self.nodes_offset(m)?;
        if offset <= 0 || size < NODE_MATRIX_SIZE {
            return Ok(None);
        }
        Ok(Some((
            self.addr.wrapping_add(offset as u32),
            size / NODE_MATRIX_SIZE,
        )))
    }
}

/// The object types, in the type table's order (`0x00468630`: bipd, vehi,
/// weap, eqip, garb, proj, scen, mach, ctrl, lifi, ssce, bloc, crea).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum ObjectType {
    Biped = 0,
    Vehicle = 1,
    Weapon = 2,
    Equipment = 3,
    Garbage = 4,
    Projectile = 5,
    Scenery = 6,
    Machine = 7,
    Control = 8,
    LightFixture = 9,
    SoundScenery = 10,
    Crate = 11,
    Creature = 12,
}

impl ObjectType {
    pub const ALL: [ObjectType; 13] = [
        ObjectType::Biped,
        ObjectType::Vehicle,
        ObjectType::Weapon,
        ObjectType::Equipment,
        ObjectType::Garbage,
        ObjectType::Projectile,
        ObjectType::Scenery,
        ObjectType::Machine,
        ObjectType::Control,
        ObjectType::LightFixture,
        ObjectType::SoundScenery,
        ObjectType::Crate,
        ObjectType::Creature,
    ];

    pub const fn from_u8(v: u8) -> Option<ObjectType> {
        if (v as usize) < Self::ALL.len() {
            Some(Self::ALL[v as usize])
        } else {
            None
        }
    }

    /// The tag group's four letters.
    pub const fn group(self) -> &'static str {
        match self {
            ObjectType::Biped => "bipd",
            ObjectType::Vehicle => "vehi",
            ObjectType::Weapon => "weap",
            ObjectType::Equipment => "eqip",
            ObjectType::Garbage => "garb",
            ObjectType::Projectile => "proj",
            ObjectType::Scenery => "scen",
            ObjectType::Machine => "mach",
            ObjectType::Control => "ctrl",
            ObjectType::LightFixture => "lifi",
            ObjectType::SoundScenery => "ssce",
            ObjectType::Crate => "bloc",
            ObjectType::Creature => "crea",
        }
    }

    /// Bipeds and vehicles: the types whose body is a [`Unit`].
    pub const fn is_unit(self) -> bool {
        matches!(self, ObjectType::Biped | ObjectType::Vehicle)
    }
}
