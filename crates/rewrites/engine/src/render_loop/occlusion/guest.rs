//! The game's view and object state as the render front-end holds it
//! while it builds a view's object render records: the visibility struct,
//! the object datum array, the render camera, the view index and the frame
//! counter (docs/graphics/occlusion-culling.md, "The read"). The native
//! renderer's stages, the scene step and the camera readers use them.
//!
//! Every address and offset is a research point from the read of the 1.0
//! XBE (labels there); no game data.

use guest_layout::memory::{GuestMemory, MemFault};

/// The main view's visibility struct (`0x00547f88`); the dynamic-light
/// views use `0x0054aa68`.
pub const MAIN_VIS: u32 = 0x0054_7f88;
/// Its view kind (0 the main view).
pub const VIS_KIND: u32 = 0x2a78;
/// Its object list pointer: `+4` the u16 count, `+8` the u16 flags, `+0xc`
/// the u32 handles.
pub const VIS_OBJECTS: u32 = 0x14;
pub const LIST_COUNT: u32 = 0x4;
pub const LIST_FLAGS: u32 = 0x8;
pub const LIST_HANDLES: u32 = 0xc;
/// The flag bit the setup's own "too small to draw" exit sets.
pub const FLAG_NOT_DRAWN: u8 = 1;

/// The object datum array pointer; its `+0x44` is the 12-byte headers,
/// whose `+8` is the object body.
pub const OBJECTS: u32 = 0x004e_0300;
pub const OBJECTS_DATA: u32 = 0x44;
pub const HEADER_SIZE: u32 = 12;
pub const HEADER_BODY: u32 = 8;
/// Body fields.
pub const BODY_PARENT: u32 = 0x14;
pub const BODY_SPHERE_CENTER: u32 = 0x30;
pub const BODY_SPHERE_RADIUS: u32 = 0x3c;

/// The render camera (0x74 bytes): position `+0`, forward `+0xc`, up
/// `+0x18`, vertical FOV `+0x28`, viewport `+0x30` (i16 top, left, bottom,
/// right), near `+0x40` (S).
pub const CAMERA: u32 = 0x004b_9da0;
/// The view index `render_game_view` stores (`[view+4]`).
pub const VIEW_INDEX: u32 = 0x004b_a050;
/// The view's local user index, `-1` for none: the argument
/// `render_view_scene` (`0x0002ba10`) keeps. Not a unit: `0x0002b990` maps
/// it to the unit it appends (`scene_extract::visibility::guest::view_player_unit`).
pub const VIEW_PLAYER: u32 = 0x004b_9ed8;
/// The frame counter `render_views` increments.
pub const FRAME: u32 = 0x004b_a034;
/// The camera's far plane (`+0x44`; `render_game_views` stores the global
/// far there at `0x002236e7`, the near at `+0x40`). The game's W-buffer
/// scale is `z_scale / far`: `0x00012a50` and `0x00013450` build the
/// program clip range `[near / far · z_scale, z_scale]` from the same
/// globals and `0x00012d72` writes it to the device's `+0x1C10` as the
/// game's own (`+0x1C18` bit 1).
pub const CAMERA_FAR: u32 = CAMERA + 0x44;

/// An object's body pointer from its handle (index in the low 16 bits).
pub fn body(m: &dyn GuestMemory, handle: u32) -> Result<u32, MemFault> {
    let data = m.u32(m.u32(OBJECTS)? + OBJECTS_DATA)?;
    m.u32(data + (handle & 0xffff) * HEADER_SIZE + HEADER_BODY)
}
