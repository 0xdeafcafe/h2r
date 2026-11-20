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
/// In it, the pointer to the leaves (8 bytes each, cluster `i16` at `+0`).
pub const STRUCTURE_LEAVES: u32 = 0x30;
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

/// Bytes below the entry `esp` the vector test's frames can reach: 0x2c
/// per recursion level to the kernel's depth limit, plus the wrapper's
/// frame, the matcher and the ring test.
pub const VECTOR_DEAD_BELOW: u32 = 0x3000;
/// Bytes of stack arguments the vector-test entries pop (they also write
/// into their own argument slots).
pub const VECTOR_ARGS: u32 = 0x10;
