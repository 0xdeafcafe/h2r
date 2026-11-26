//! Havok guest offsets: the world, islands, entities, motions, shapes,
//! constraints and the memory system's pool. Addresses are retail's, with
//! the function that shows each (docs/engine/havok-runtime.md,
//! docs/engine/havok-step.md). Telemetry re-exports these as
//! `live_telemetry::layout`; no runtime dependency.

// The Havok world as `havok.bodies` and `havok.contacts` walk it
// (docs/engine/havok-runtime.md; addresses there). Arrays are Havok's
// (pointer, count, capacity with bit 31 "not owned").
/// Active simulation islands (`0x0030ccd0` moves one here from the
/// inactive array).
pub const WORLD_ACTIVE_ISLANDS: u32 = 0x08;
pub const WORLD_INACTIVE_ISLANDS: u32 = 0x14;
/// The fixed island (the level, static bodies), built at `0x0031071a`.
pub const WORLD_FIXED_ISLAND: u32 = 0x2c;
/// An island's entities (`0x001c4082`) and constraints (`0x00316cf0`).
pub const ISLAND_ENTITIES: u32 = 0x3c;
pub const ISLAND_CONSTRAINTS: u32 = 0x58;
/// An entity's properties: (key, value) pairs of 8 bytes (`0x001c3f33`).
pub const ENTITY_PROPERTIES: u32 = 0x30;
/// The property whose value is the physics component index.
pub const PROPERTY_PHYSICS_COMPONENT: u32 = 0x2001;
/// An entity's material friction and restitution (`0x00318d1c`).
pub const ENTITY_FRICTION: u32 = 0x4c;
pub const ENTITY_RESTITUTION: u32 = 0x50;
/// A physics component's object datum handle (`0x001c3fa4`).
pub const COMPONENT_OBJECT: u32 = 0x08;
/// Motion: inverse mass (`0x00318e1c`) and angular velocity (`0x001c4f51`).
pub const MOTION_INV_MASS: u32 = 0x2c;
pub const MOTION_ANGULAR_VELOCITY: u32 = 0x50;
/// The contact constraint's vtable (`0x0031c00f`): the constraint is
/// embedded at contact manager `+0x10` (`0x00319110`).
pub const CONTACT_CONSTRAINT_VTABLE: u32 = 0x0041_53c8;
pub const CONSTRAINT_ENTITY_A: u32 = 0x0c;
pub const CONSTRAINT_ENTITY_B: u32 = 0x10;
/// Contact points (`0x0031bedb`, element `0x20`): position, then the
/// normal; the separation distance in the position's `w` (`0x0031f810`).
pub const CONTACT_POINTS: u32 = 0x34;
pub const CONTACT_POINT_SIZE: u32 = 0x20;
pub const POINT_DISTANCE: u32 = 0x0c;
pub const POINT_NORMAL: u32 = 0x10;
/// Per-point properties (`0x0031bf1f`, element `0x1c`), in point order: the
/// impulse the solver applied (`0x0031e530`), friction, restitution
/// (`0x00318d3c`).
pub const CONTACT_PROPERTIES: u32 = 0x90;
pub const CONTACT_PROPERTY_SIZE: u32 = 0x1c;
pub const PROPERTY_IMPULSE: u32 = 0x00;
pub const PROPERTY_FRICTION: u32 = 0x08;
pub const PROPERTY_RESTITUTION: u32 = 0x0c;
/// A physics component's rigid body count, next to `COMPONENT_BODIES`
/// (the release loop at `0x001d022a`); entries `0x60` apart (`0x001d08ec`).
pub const COMPONENT_BODY_COUNT: u32 = 0x74;
pub const COMPONENT_BODY_SIZE: u32 = 0x60;
/// An entity's collidable shape: the world object constructor copies the
/// construction info's shape here and takes a reference (`0x003125ca`,
/// `0x00312609`); the biped's info gets its pill at `0x001d71bc`.
pub const ENTITY_SHAPE: u32 = 0x0c;
/// A shape's type is its virtual `+0x14`; these are the getters (each a
/// `mov eax, type; ret`) for the types `havok.bodies` decodes. Halo's tag
/// shapes (vtables `0x00455500`, `0x004554c0`, `0x00455498`) and Havok's
/// (`0x00455458`, `0x00455418`, `0x004553f0`) share them.
pub const SHAPE_TYPE_SLOT: u32 = 0x14;
pub const SHAPE_TYPE_SPHERE_FN: u32 = 0x0009_cd90;
pub const SHAPE_TYPE_CAPSULE_FN: u32 = 0x0009_b910;
pub const SHAPE_TYPE_TRANSFORM_FN: u32 = 0x002d_e7a0;
/// A sphere's or capsule's radius (`0x002ded6f`, `0x002de0ff`).
pub const SHAPE_RADIUS: u32 = 0x0c;
/// A capsule's two end points, each with the radius in `w` (the
/// constructor `0x002de0a0`; the bounding box `0x002de0f0`).
pub const CAPSULE_A: u32 = 0x10;
pub const CAPSULE_B: u32 = 0x20;
/// A transform shape's child (`0x002de870`) and its transform (`+0x10`,
/// `0x002de85f`), the translation in the last column (`0x001d6fe3`).
pub const TRANSFORM_CHILD: u32 = 0x0c;
pub const TRANSFORM_TRANSLATION: u32 = 0x40;

// The memory system (docs/engine/havok-step.md, "Allocator").
/// The dispatcher `[0x00480118]` points at (`0x001473b0`), the directory
/// pool, the class pool, the borrowed block's directory pool, and the
/// overflow object's slot. `0x00146a20` builds the first three.
pub const MEMORY_SYSTEM: u32 = 0x0048_0118;
pub const DISPATCH_OBJECT: u32 = 0x0047_98b0;
pub const POOL_MODE: u32 = 0x0047_9890;
pub const DIRECTORY_POOL: u32 = 0x0047_9894;
pub const CLASS_POOL: u32 = 0x0047_9898;
pub const BORROWED_POOL: u32 = 0x0047_989c;
pub const OVERFLOW_POOL: u32 = 0x0047_98a0;
/// The directory pool's vtable and the class pool's.
pub const DIRECTORY_VTABLE: u32 = 0x0045_76a4;
pub const CLASS_VTABLE: u32 = 0x0045_7620;
/// Every pool object: vtable `+0x00`, reference count `+0x04`. A LIFO
/// descriptor (cursor, remaining, block, payload) is `+0x08..+0x18`.
pub const POOL_REFS: u32 = 0x04;
pub const POOL_LIFO_CURSOR: u32 = 0x08;
pub const POOL_LIFO_REMAINING: u32 = 0x0c;
pub const POOL_LIFO_BLOCK: u32 = 0x10;
pub const POOL_LIFO_PAYLOAD: u32 = 0x14;
/// Directory pool: base `+0x18`, bytes `+0x1c`, live count `+0x20`. The
/// directory grows down from the end, (offset, bytes) per allocation.
pub const DIR_BASE: u32 = 0x18;
pub const DIR_BYTES: u32 = 0x1c;
pub const DIR_COUNT: u32 = 0x20;
/// Class pool: bump page cursor `+0x20` and limit `+0x1c`, free-list heads
/// `+0x24` (a class each, from 1), class sizes `+0x68`, the size to class
/// table `+0xac` (bytes, sizes up to `0x200`), the large table `+0x2b0`,
/// pages carved `+0x2d0`, allocations per class `+0x2e8`, the page count
/// `+0xf0340`, charged blocks `+0xf0344` and free blocks `+0xf0388` per class.
pub const CLASS_PAGE_BASE: u32 = 0x18;
pub const CLASS_PAGE_LIMIT: u32 = 0x1c;
pub const CLASS_PAGE_CURSOR: u32 = 0x20;
pub const CLASS_FREE_HEADS: u32 = 0x24;
pub const CLASS_SIZES: u32 = 0x68;
pub const CLASS_BY_SIZE: u32 = 0xac;
pub const CLASS_BY_KILOBYTE: u32 = 0x2b0;
pub const CLASS_PAGES_CARVED: u32 = 0x2d0;
pub const CLASS_ALLOCATIONS: u32 = 0x2e8;
pub const CLASS_PAGE_COUNT: u32 = 0xf0340;
pub const CLASS_CHARGED: u32 = 0xf0344;
pub const CLASS_FREE: u32 = 0xf0388;
/// First page, page size, page limit, and the class count (index 0 unused).
pub const CLASS_PAGES: u32 = 0x4340;
pub const CLASS_PAGE_BYTES: u32 = 0x2000;
pub const CLASS_PAGE_MAX: u32 = 0x76;
pub const CLASSES: u32 = 17;

// Havok arrays (`0x002d91d0` grows them): data pointer, count, then the
// capacity with bit 31 set when the array does not own its data.
pub const ARRAY_DATA: u32 = 0x00;
pub const ARRAY_COUNT: u32 = 0x04;
pub const ARRAY_CAPACITY: u32 = 0x08;
pub const ARRAY_NOT_OWNED: u32 = 0x8000_0000;
