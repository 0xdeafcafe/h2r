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
        /// Header flags ([`header`]): the walks select objects by them
        /// (docs/engine/sim-rewrite-objects.md section 2).
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
        /// Body flags. Bit 8 ([`IN_PARTITION`]): registered in the spatial
        /// partition (the bounds update `0x000bdef0` reads it). The node
        /// matrix update `0x000bd090` clears bit 30.
        BODY_FLAGS body_flags @ 0x04: u32,
        /// The next child of this object's parent, or none.
        NEXT_SIBLING next_sibling @ 0x0c: Datum,
        /// The first attached child, or none. The walks visit a parent,
        /// then its children through [`Object::NEXT_SIBLING`].
        FIRST_CHILD first_child @ 0x10: Datum,
        /// The object this one is attached to; its transform is then
        /// parent-relative.
        PARENT parent @ 0x14: Datum,
        /// The scenario placement index (`i16`, -1 for objects the scenario
        /// did not place). Confirmed: `object_new` copies it from the
        /// placement data's `+0x10` (`0x000b7c87`).
        PLACEMENT_INDEX placement_index @ 0x1a: i16,
        /// Where the object is in the structure BSP: the 8-byte location
        /// record (leaf, cluster, BSP index; docs/engine/collision-queries.md
        /// entry B). The early-mover wake `0x00109580` queries the
        /// partition with it.
        LOCATION location @ 0x28: [u8; 8],
        /// The bounding sphere's centre (world space) and radius, written by
        /// the bounds update `0x000bdef0`.
        BOUND_CENTER bound_center @ 0x30: [f32; 3],
        BOUND_RADIUS bound_radius @ 0x3c: f32,
        /// The sphere the spatial partition last registered: rewritten
        /// only when the object is relinked (`0x000bdef0`).
        PARTITION_CENTER partition_center @ 0x40: [f32; 3],
        PARTITION_RADIUS partition_radius @ 0x4c: f32,
        /// The second sphere: centre offset by the definition's
        /// [`ObjectDefinition::DYNAMIC_OFFSET`], radius scaled by its
        /// [`ObjectDefinition::DYNAMIC_SCALE`] (`0x000bdef0`).
        DYNAMIC_CENTER dynamic_center @ 0x50: [f32; 3],
        DYNAMIC_RADIUS dynamic_radius @ 0x5c: f32,
        POSITION position @ 0x64: [f32; 3],
        FORWARD forward @ 0x70: [f32; 3],
        UP up @ 0x7c: [f32; 3],
        /// Translational velocity in world units a second, not a tick
        /// (confirmed on the projectile update, `0x000f8ee2` on).
        VELOCITY velocity @ 0x88: [f32; 3],
        /// The object's scale; the bounds update multiplies the
        /// definition's radius by it.
        SCALE scale @ 0xa0: f32,
        /// likely: the type again, as a byte (the header's
        /// [`ObjectHeader::KIND`] is the one the walks read).
        BODY_KIND body_kind @ 0xaa: u8,
        /// The netgame equipment entry that spawned the object (`i8`, -1
        /// none). Confirmed: the spawner `0x0015aad0` writes it.
        NETGAME_EQUIPMENT netgame_equipment @ 0xaf: i8,
        /// Ticks the object stays awake with nothing to do: the pre-update
        /// `0x000bc470` decrements it and keeps the object awake while it
        /// is non-zero.
        SLEEP_TIMER sleep_timer @ 0xb3: u8,
        /// The physics component index (-1 none) into
        /// [`crate::global::PHYSICS_COMPONENTS`] (confirmed, `0x001d08e0`).
        PHYSICS physics @ 0xb4: u32,
        /// The early mover whose sphere woke this object (`0x00109580`
        /// stores it).
        WOKEN_BY woken_by @ 0xb8: Datum,
        /// Physics flags ([`physics_flags`]): the wake latch and its
        /// one-tick copy, the early-mover list, the component.
        PHYSICS_FLAGS physics_flags @ 0xc0: u8,
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
        /// The attachments block's offset from the body (`0x000bef30`
        /// refreshes it).
        ATTACHMENTS_OFFSET attachments_offset @ 0x11e: i16,
        /// Where the animation manager is: an offset from the body, -1 none.
        /// Confirmed: `object_new` (`0x000b7b40`) builds it there.
        ANIMATION_MANAGER animation_manager @ 0x12a: i16,
    }
}

/// [`Object::BODY_FLAGS`] bit 8: registered in the spatial partition.
pub const IN_PARTITION: u32 = 1 << 8;

/// Bits of [`ObjectHeader::FLAGS`]. Confirmed from the walks
/// `0x000b6f10`, `0x000b7060`, `0x000b7150` and the per-object updates
/// `0x000bc470`, `0x000bc820` (docs/engine/sim-rewrite-objects.md
/// section 2; docs/engine/object-update.md).
pub mod header {
    /// Live and simulated: every walk requires it.
    pub const ACTIVE: u8 = 0x01;
    /// Updated this tick. Set up the parent chain by the wake
    /// `0x000b7360`; cleared when an update has nothing left to do.
    pub const AWAKE: u8 = 0x02;
    /// Due a post-physics update: cleared by the pre-update, set by
    /// `0x000bba20`. The post walk visits `flags & (POST | NEW) == POST`.
    pub const POST: u8 = 0x04;
    /// Made or woken this tick: out of the pre and post walks; finalize
    /// clears it and runs both updates.
    pub const NEW: u8 = 0x08;
    /// Deleted by finalize's second pass.
    pub const DELETE: u8 = 0x10;
    /// The pre-update returns at once while it is set; finalize clears it
    /// on every live header.
    pub const DEFER: u8 = 0x20;
    /// Has attachments the post-update refreshes.
    pub const ATTACH: u8 = 0x40;
    /// Attached to a parent: a wake goes to the root ancestor.
    pub const CHILD: u8 = 0x80;
}

/// Bits of [`Object::PHYSICS_FLAGS`] (the walks and `0x000bc820`).
pub mod physics_flags {
    /// The wake latch's copy from the previous tick (`0x000bc820` copies
    /// [`WOKEN`] here, then clears it).
    pub const WOKEN_LAST: u8 = 1 << 1;
    /// Woken by an early mover this tick (`0x00109580`).
    pub const WOKEN: u8 = 1 << 2;
    /// On the early-mover list: the slot passes skip it.
    pub const EARLY_MOVER: u8 = 1 << 3;
    /// The physics component is active.
    pub const COMPONENT_ACTIVE: u8 = 1 << 6;
}

crate::layout! {
    /// The start of an object definition tag's data (`obje` and every
    /// type that extends it): the bounds the bounds update `0x000bdef0`
    /// reads.
    pub struct ObjectDefinition: 0x30 {
        /// likely: definition flags; bit 12 picks which sphere the
        /// partition registers.
        FLAGS flags @ 0x02: u16,
        /// The bounding radius before the object's scale.
        BOUNDING_RADIUS bounding_radius @ 0x04: f32,
        /// The bounding sphere's centre in the root node's space.
        BOUNDING_OFFSET bounding_offset @ 0x08: [f32; 3],
        /// The second sphere's radius scale (used when positive).
        DYNAMIC_SCALE dynamic_scale @ 0x20: f32,
        /// The second sphere's centre offset.
        DYNAMIC_OFFSET dynamic_offset @ 0x24: [f32; 3],
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
