//! Guest addresses and record offsets the collision adapters read and
//! write (docs/engine/collision-queries.md). Research points only: no
//! values from the image are copied here.

/// `i16`: the active structure BSP index (`-1` before a structure loads).
pub const BSP_INDEX: u32 = 0x0046_86c4;
/// The structure's collision BSP: the pointer the engine passes as a BSP
/// (its eight `{count, pointer}` blocks).
pub const STRUCTURE_BSP: u32 = 0x004e_033c;
/// The active structure BSP tag.
pub const ACTIVE_STRUCTURE: u32 = 0x004e_0348;
/// Structure leaf block count, used to bound the decoded cluster table.
pub const STRUCTURE_LEAF_COUNT: u32 = 0x2c;
/// In it, the pointer to the leaves (8 bytes each, cluster `i16` at `+0`).
pub const STRUCTURE_LEAVES: u32 = 0x30;
/// Where the point-to-leaf call inside the location query returns.
pub const LOCATION_AFTER_POINT: u32 = 0x0011_befb;
/// Six rows of three `i16`: the `u` and `v` axes of each projection.
pub const PROJECTION: u32 = 0x0044_0b94;
/// The binary32 zero the matcher compares a normal against.
pub const ZERO: u32 = 0x0045_dbd8;
/// The binary32 one the vector test clamps its limit to.
pub const ONE: u32 = 0x0045_dbc0;

/// The vector test's result record R (entries C and D).
pub mod record {
    /// `f32`: the hit fraction; `max(limit, 0)` before the walk.
    pub const FRACTION: u32 = 0x00;
    /// The hit plane's guest address.
    pub const PLANE: u32 = 0x04;
    /// The leaf the surface was found in, then the surface index.
    pub const LEAF: u32 = 0x08;
    pub const SURFACE: u32 = 0x0c;
    /// The surface's plane word, sign-extended (bit 15: flipped).
    pub const SURFACE_PLANE: u32 = 0x10;
    /// Surface flags `u8`, breakable index `u8`, material `i16`.
    pub const FLAGS: u32 = 0x14;
    pub const BREAKABLE: u32 = 0x15;
    pub const MATERIAL: u32 = 0x16;
    /// `u32` leaves listed, then up to 256 leaf indices.
    pub const COUNT: u32 = 0x18;
    pub const LEAVES: u32 = 0x1c;
    /// The last leaf visited once the list is full.
    pub const OVERFLOW: u32 = 0x418;
    pub const SIZE: u32 = 0x41c;
}

/// The vector test's 40-byte context (entry D's first argument; entry C
/// builds one in its own frame).
pub mod context {
    pub const FLAGS: u32 = 0x00;
    pub const BSP: u32 = 0x04;
    /// `u16`.
    pub const BREAKABLE_COUNT: u32 = 0x08;
    pub const MASK: u32 = 0x0c;
    pub const START: u32 = 0x10;
    pub const DELTA: u32 = 0x14;
    pub const RECORD: u32 = 0x18;
    pub const PREV_LEAF: u32 = 0x1c;
    /// `u8`.
    pub const PREV_KIND: u32 = 0x20;
    pub const PLANE: u32 = 0x24;
    pub const SIZE: u32 = 0x28;
}

/// A near-child call enters 0x28 bytes below its parent. The deepest leaf
/// uses another 0x30 to enter the matcher, 0x48 to call the ring test and
/// 0x18 in that test. A depth beyond this limit declines before commit.
pub const CONTEXT_DEAD_BELOW: u32 = (sim_collision::bsp::MAX_DEPTH - 1) * 0x28 + 0x90;
/// The vector wrapper reserves its 0x28-byte context and calls the walker
/// with four words and a return address below it.
pub const VECTOR_DEAD_BELOW: u32 = 0x3c + CONTEXT_DEAD_BELOW;
/// Bytes of stack arguments the vector-test entries pop (they also write
/// into their own argument slots).
pub const VECTOR_ARGS: u32 = 0x10;

/// `u8`: set while the object visit floods (entry H sets it and clears it).
pub const FLOOD_ACTIVE: u32 = 0x004e_7411;
/// `u32`: the cluster flood's epoch. The flood (G) marks under the epoch its
/// caller set; the visit (H) bumps it before flooding.
pub const FLOOD_EPOCH: u32 = 0x004e_7414;
/// One `u32` mark per cluster.
pub const CLUSTER_MARKS: u32 = 0x004e_7418;
/// `u8`: set while the object visit walks its lists.
pub const VISIT_ACTIVE: u32 = 0x004d_e2f8;
/// `u32`: the object visit's generation, bumped once per visit.
pub const VISIT_GENERATION: u32 = 0x004d_e2fc;
/// One `u32` mark per object slot (the handle's low half).
pub const OBJECT_MARKS: u32 = 0x004d_e300;
/// Object mark slots the native visit writes. A higher slot's mark would
/// reach the structure globals at `0x004e_033c`; the visit declines it.
pub const OBJECT_MARK_SLOTS: u32 = 0x800;
/// The collideable reference list: the per-cluster head table pointer and
/// the datum array header pointer (`0x000b8940`).
pub const COLLIDEABLE_HEADS: u32 = 0x004d_e2e0;
pub const COLLIDEABLE_ARRAY: u32 = 0x004d_e2e4;
/// The second reference list (`0x000b8a10`).
pub const OTHER_HEADS: u32 = 0x004d_e2d4;
pub const OTHER_ARRAY: u32 = 0x004d_e2d8;

/// Bytes of stack arguments the flood pops; it reuses two of its argument
/// slots as locals.
pub const FLOOD_ARGS: u32 = 0x14;
/// Bytes of stack arguments the visit pops; it rewrites two of them.
pub const VISIT_ARGS: u32 = 0x1c;

/// The stack the flood writes below its entry `esp`: its five saved
/// registers, 0x2c per nested flood, and 0x44c under a frame that runs the
/// portal test (0x420 of locals, the polygon test's frame). `deepest` is
/// the deepest nesting that ran a test.
pub const fn flood_dead_below(deepest: Option<u32>) -> u32 {
    match deepest {
        Some(k) => 0x14 + 0x2c * k + 0x44c,
        None => 0x14,
    }
}

/// Every flood the native kernel admits stays within this window.
pub const FLOOD_DEAD_BELOW: u32 =
    flood_dead_below(Some(sim_collision::flood::MAX_CLUSTERS - 1));
/// The visit's 0x430-byte frame, then 0x18 for a list-head call or for the
/// flood's arguments and return address.
pub const fn visit_dead_below(flood: Option<Option<u32>>) -> u32 {
    match flood {
        Some(deepest) => 0x448 + flood_dead_below(deepest),
        None => 0x448,
    }
}
pub const VISIT_DEAD_BELOW: u32 = 0x448 + FLOOD_DEAD_BELOW;
