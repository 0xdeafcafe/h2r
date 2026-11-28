//! The target select by index `0x00014bc0` in Rust
//! (docs/graphics/retire-d3d-display.md, "The post and the screen
//! effects"): the index's colour and depth, the scene's select
//! `0x0001cd30` ([`native_targets::cut_select`]) and the viewport, so the
//! bind makes no guest call.
//!
//! The guest's body (Code, `0x00014bc0..0x00014eca`, `ret 8`): AL keeps
//! the depth (0 drops it), the stack's words are the index and a level.
//!
//! 1. The index's surfaces through its byte table `0x00014f30` and jump
//!    table `0x00014ed0` ([`surfaces`]).
//! 2. `0x0001cd30(esi colour, eax depth or 0, flag)`.
//! 3. The viewport (`0x00014d47..0x00014ebe`, [`viewport`]), then
//!    `D3DDevice_SetViewport` (`0x003f8100`).
//!
//! A bind this does not cover is an error naming it: `index` (an index
//! the tables send nowhere), `colour` (no colour surface to size the
//! viewport by), `unread`, `unwritable`, `device`.

use engine::memory::{GuestMemory, MemFault};

use crate::native_targets::{self, Host};
use crate::view_setup;

/// `0x00014bc0`: AL the depth's keep, stack (index word, level word).
pub const BIND_TARGET: u32 = 0x0001_4bc0;

/// The device's colour and depth buffers, and the colour of its second
/// buffer (`0x00014be5`, `0x00014bfb`).
pub const DEVICE_COLOUR: u32 = 0x0050_935c;
pub const DEVICE_SECOND: u32 = 0x0050_9360;
pub const DEVICE_DEPTH: u32 = 0x0050_9364;
/// The colour over the depth buffer's memory (index 3), the 64 x 64
/// textures' level 0 (`0x10`/`0x1d`, `0x11`), the static surface
/// index 9 points at target 9's memory, and the six slots `0x21..0x26`
/// with their depth.
pub const DEPTH_ALIAS: u32 = 0x0050_9370;
pub const SMALL_A: u32 = 0x0050_9378;
pub const SMALL_B: u32 = 0x0050_9380;
pub const STATIC: u32 = 0x0050_9384;
pub const SLOTS: u32 = 0x0050_9390;
pub const SLOT_DEPTH: u32 = 0x0050_93a8;

type Cause = &'static str;

fn unread(_: MemFault) -> Cause {
    "unread"
}

fn word(mem: &dyn GuestMemory, at: u32) -> Result<u32, Cause> {
    mem.u32(at).map_err(unread)
}

/// What an index binds: the colour, the depth before AL's mask, and the
/// flag byte `0x0001cd30` is passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surfaces {
    pub colour: u32,
    pub depth: u32,
    pub flag: u8,
}

/// `0x0001dd20(eax target, edx level)`: the record's surface at `level`
/// when it has a texture (`+0x88`), is not off (`+0x95`) and `level` is
/// below its count (`+0x78`), else null.
pub fn level(mem: &dyn GuestMemory, target: u32, level: i32) -> Result<u32, Cause> {
    let r = native_targets::TARGETS.wrapping_add(target.wrapping_mul(native_targets::TARGET_STRIDE));
    if word(mem, r.wrapping_add(0x88))? == 0 || mem.u8(r.wrapping_add(0x95)).map_err(unread)? != 0 {
        return Ok(0);
    }
    if level < 0 || level >= mem.i32(r.wrapping_add(0x78)).map_err(unread)? {
        return Ok(0);
    }
    Ok(r.wrapping_add(0x18).wrapping_add(0x18 * level as u32))
}

/// Step 1 for `index` and `level`. Index 9 also writes target 9's data
/// and size into the static surface (`0x00014c61..0x00014c86`).
pub fn surfaces(mem: &mut dyn GuestMemory, index: i32, lvl: i32) -> Result<Surfaces, Cause> {
    let cached = |mem: &dyn GuestMemory, t: u32| view_setup::cached(mem, t);
    let s = |colour, depth, flag| Ok(Surfaces { colour, depth, flag });
    let depth = word(&*mem, DEVICE_DEPTH);
    match index {
        0 => s(word(&*mem, DEVICE_COLOUR)?, depth?, 1),
        3 => s(word(&*mem, DEPTH_ALIAS)?, depth?, 1),
        4 => s(word(&*mem, DEVICE_SECOND)?, 0, 1),
        1 | 5..=8 | 0xf | 0x12 => s(cached(&*mem, index as u32)?, depth?, 0),
        9 => {
            let colour = word(&*mem, STATIC)?;
            let target = cached(&*mem, 9)?;
            let size = word(&*mem, target.wrapping_add(0x10))?;
            let data = word(&*mem, target.wrapping_add(4))?;
            mem.set_u32(colour.wrapping_add(0x10), size).map_err(|_| "unwritable")?;
            mem.set_u32(colour.wrapping_add(4), data).map_err(|_| "unwritable")?;
            s(colour, target, 0)
        }
        0xa..=0xc | 0x13..=0x16 | 0x19..=0x1b => s(cached(&*mem, index as u32)?, 0, 0),
        0xd => s(level(&*mem, 0xd, lvl)?, 0, 0),
        0xe | 0x17 => s(0, 0, 0),
        0x10 | 0x1d => s(word(&*mem, SMALL_A)?, 0, 0),
        0x11 => s(word(&*mem, SMALL_B)?, 0, 0),
        0x21..=0x26 => s(word(&*mem, SLOTS + 4 * (index - 0x21) as u32)?, word(&*mem, SLOT_DEPTH)?, 0),
        _ => Err("index"),
    }
}

/// `GetDesc`'s level-0 size of the surface at `surface` (`0x003fdb20`,
/// `0x003fd9c0`): a linear one's `+0x10` (width and height less one, 12
/// bits each); a swizzled one's (`+0x10` 0) from its format word `+0xc`
/// (`1 << USize`, `1 << VSize`).
pub fn size(mem: &dyn GuestMemory, surface: u32) -> Result<(u32, u32), Cause> {
    let size = word(mem, surface.wrapping_add(0x10))?;
    if size != 0 {
        return Ok(((size & 0xfff) + 1, (size >> 12 & 0xfff) + 1));
    }
    let format = word(mem, surface.wrapping_add(0xc))?;
    Ok((1 << (format >> 20 & 0xf), 1 << (format >> 24 & 0xf)))
}

/// Step 3: 128 square for context kind 2; the window path
/// ([`view_setup::viewport`] over the context) for indices 0, 1, 2 and
/// `0x12` and the screen target; any other the colour's whole surface.
pub fn viewport(mem: &dyn GuestMemory, index: i32, colour: u32) -> Result<[u32; 6], Cause> {
    let one = word(mem, 0x0045_dbc0)?;
    let context = view_setup::CONTEXT;
    let kind = mem.u16(context.wrapping_add(2)).map_err(unread)?;
    if kind == 2 {
        return Ok([0, 0, 0x80, 0x80, 0, one]);
    }
    if matches!(index, 0 | 1 | 2 | 0x12) || view_setup::screen_target(mem)? == index {
        return view_setup::viewport(mem, context, index as u32);
    }
    if colour == 0 {
        return Err("colour");
    }
    let (w, h) = size(mem, colour)?;
    Ok([0, 0, w, h, 0, one])
}

/// `0x00014bc0(index, level)` with AL `keep_depth`, through `host`, the
/// viewport staged at `scratch`.
pub fn bind(mem: &mut dyn GuestMemory, host: &dyn Host, index: i32, lvl: i32, keep_depth: bool, scratch: u32) -> Result<(), Cause> {
    let s = surfaces(mem, index, lvl)?;
    let depth = if keep_depth { s.depth } else { 0 };
    native_targets::cut_select(mem, host, s.colour, depth, s.flag)?;
    let v = viewport(&*mem, index, s.colour)?;
    let staged: Vec<u8> = v.iter().flat_map(|w| w.to_le_bytes()).collect();
    mem.write(scratch, &staged).map_err(|_| "unwritable")?;
    if !host.viewport(mem, scratch) {
        return Err("device");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use engine::memory::FlatMemory;

    use super::*;
    use crate::native_targets::Bind;

    #[derive(Default)]
    struct Device {
        binds: Mutex<Vec<(u32, u32)>>,
        viewports: Mutex<Vec<[u32; 6]>>,
    }

    impl Host for Device {
        fn bind(&self, _: &mut dyn GuestMemory, _: Bind, colour: u32, depth: u32) -> bool {
            self.binds.lock().unwrap().push((colour, depth));
            true
        }
        fn clear(&self, _: &mut dyn GuestMemory, _: u32, _: u32, _: u32, _: u32) -> bool {
            true
        }
        fn bound(&self) -> Option<[u32; 8]> {
            None
        }
        fn viewport(&self, mem: &mut dyn GuestMemory, at: u32) -> bool {
            self.viewports.lock().unwrap().push(std::array::from_fn(|i| mem.u32(at + 4 * i as u32).unwrap()));
            true
        }
    }

    const COLOUR: u32 = 0x0060_0000;
    const DEPTH: u32 = 0x0060_0040;
    const SCRATCH: u32 = 0x0060_1000;
    const ONE: u32 = 0x3f80_0000;

    fn target(n: u32) -> u32 {
        native_targets::TARGETS + n * native_targets::TARGET_STRIDE
    }

    /// A 640 x 480 window in the context, target `0x13` live with a
    /// linear 256 x 128 surface, target `0xd` with two levels.
    fn world() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
        m.set_u32(0x0045_dbc0, ONE).unwrap();
        m.set_u32(DEVICE_COLOUR, COLOUR).unwrap();
        m.set_u32(DEVICE_DEPTH, DEPTH).unwrap();
        let c = view_setup::CONTEXT;
        m.set_u16(c + 0x4c, 480).unwrap();
        m.set_u16(c + 0x4e, 640).unwrap();
        for (n, levels) in [(0x13, 1), (0xd, 2)] {
            m.set_u32(target(n) + 0x88, 1).unwrap();
            m.set_u32(target(n) + 0x78, levels).unwrap();
        }
        m.set_u32(target(0x13) + 0x18 + 0x10, 255 | 127 << 12).unwrap();
        m
    }

    #[test]
    fn each_index_binds_its_tables_surfaces() {
        let mut m = world();
        assert_eq!(surfaces(&mut m, 0, 0), Ok(Surfaces { colour: COLOUR, depth: DEPTH, flag: 1 }));
        assert_eq!(surfaces(&mut m, 0x13, 0), Ok(Surfaces { colour: target(0x13) + 0x18, depth: 0, flag: 0 }));
        assert_eq!(surfaces(&mut m, 0x12, 0), Ok(Surfaces { colour: 0, depth: DEPTH, flag: 0 }), "an unmade target is null");
        assert_eq!(surfaces(&mut m, 0xd, 1).unwrap().colour, target(0xd) + 0x30, "level 1");
        assert_eq!(surfaces(&mut m, 0xd, 2).unwrap().colour, 0, "past the count");
        assert_eq!(surfaces(&mut m, 0xe, 0).unwrap().colour, 0);
        for index in [2, 0x18, 0x1c, 0x1e, 0x27, -1] {
            assert_eq!(surfaces(&mut m, index, 0), Err("index"), "{index:#x}");
        }
    }

    #[test]
    fn index_9_points_the_static_surface_at_target_9() {
        let mut m = world();
        let fixed = 0x0048_4f50;
        m.set_u32(STATIC, fixed).unwrap();
        m.set_u32(target(9) + 0x88, 1).unwrap();
        m.set_u32(target(9) + 0x78, 1).unwrap();
        m.set_u32(target(9) + 0x18 + 4, 0x0123_4000).unwrap();
        m.set_u32(target(9) + 0x18 + 0x10, 0x0011_f27f).unwrap();
        assert_eq!(surfaces(&mut m, 9, 0), Ok(Surfaces { colour: fixed, depth: target(9) + 0x18, flag: 0 }));
        assert_eq!((m.u32(fixed + 4).unwrap(), m.u32(fixed + 0x10).unwrap()), (0x0123_4000, 0x0011_f27f));
    }

    #[test]
    fn the_viewport_is_the_window_for_the_view_and_the_surface_for_others() {
        let mut m = world();
        let dev = Device::default();
        bind(&mut m, &dev, 0, 0, true, SCRATCH).unwrap();
        bind(&mut m, &dev, 0x13, 0, true, SCRATCH).unwrap();
        bind(&mut m, &dev, 0, 0, false, SCRATCH).unwrap();
        assert_eq!(*dev.binds.lock().unwrap(), [(COLOUR, DEPTH), (target(0x13) + 0x18, 0), (COLOUR, 0)], "AL 0 drops the depth");
        assert_eq!(*dev.viewports.lock().unwrap(), [[0, 0, 640, 480, 0, ONE], [0, 0, 256, 128, 0, ONE], [0, 0, 640, 480, 0, ONE]]);
        assert_eq!(bind(&mut m, &dev, 0x17, 0, true, SCRATCH), Err("colour"), "nothing to size");
        m.set_u16(view_setup::CONTEXT + 2, 2).unwrap();
        assert_eq!(viewport(&m, 0x17, 0), Ok([0, 0, 0x80, 0x80, 0, ONE]), "kind 2 first");
    }

    #[test]
    fn a_swizzled_surface_is_sized_by_its_format() {
        let mut m = world();
        m.set_u32(COLOUR + 0xc, 6 << 20 | 5 << 24).unwrap();
        assert_eq!(size(&m, COLOUR), Ok((64, 32)));
    }
}
