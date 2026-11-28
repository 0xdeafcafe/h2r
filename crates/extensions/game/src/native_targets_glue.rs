//! The host of `render_bridge::native_targets`: the scene's target select,
//! clear and view copy as frame ops (docs/graphics/native-renderer.md,
//! "How a frame flows"), and the frame tail's overlay. There is no device:
//! a guest state write has no reader and is taken as made, and the state
//! the draws inherit is read from what the guest keeps in its own memory
//! ([`render_state`], [`constant`], [`stage_palette`]).
//!
//! The stage glue's shared steps live here too: [`push`] places a typed
//! stage at its slot, [`encoded`] wraps encoder commands as stage work and
//! [`target_ref`] names a guest surface header as a frame target.

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use frame_types::{Slot, SlotAt, TargetRef};
use native_render::{FrameOp, Stage, StageWork, frame_builder};
use render_bridge::native_targets::{self, Bind, Host};

/// `stage` at `slot` of the guest view the frame step is in
/// (`engine::render_loop::occlusion::guest::VIEW_INDEX`, 0 outside one).
pub fn push(mem: &dyn GuestMemory, slot: Slot, stage: impl Stage + 'static) {
    let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).map_or(0, |v| u8::try_from(v).unwrap_or(0));
    frame_builder().push(FrameOp::Stage { slot: SlotAt { slot, view }, stage: Box::new(stage) });
}

/// Commands for the frame's encoder as stage work: one range, one draw in
/// one pass. `commands` runs twice (once under validation, once for real),
/// so it records only.
pub fn encoded(commands: impl Fn(&mut wgpu::CommandEncoder) + Send + 'static) -> StageWork {
    StageWork::Encoded(gfx::external::ExternalEncoded { ranges: 1, draws: 1, passes: 1, commands: Box::new(commands) })
}

/// The frame target behind guest surface `header`: 0 is none; a header
/// with no game role (`native_targets::role`) is the device's own back
/// buffer (`colour`) or depth; a game role is its own target
/// (`TargetRole::target`), which the frame graph makes at the guest's
/// size for it (`native_render::frame_graph::targets::GAME`).
pub fn target_ref(mem: &dyn GuestMemory, header: u32, colour: bool) -> TargetRef {
    if header == 0 {
        return TargetRef::None;
    }
    match native_targets::role(header, &|a| mem.u32(a).ok()).and_then(|r| r.target()) {
        Some(target) => target,
        None if colour => TargetRef::SceneColour,
        None => TargetRef::SceneDepth,
    }
}

/// The guest virtual address of a resource's `Data` word: the Xbox
/// contiguous mapping, `0x80000000 | (data & 0x0FFFFFFF)`.
pub fn data_address(data: u32) -> u32 {
    0x8000_0000 | (data & 0x0FFF_FFFF)
}

/// D3D's vertex constant shadow: the 192 registers in hardware order
/// (register + 96), 16 bytes each, as `SetVertexShaderConstant1` /
/// `4` / `NotInline` (`0x003f71d0`, `0x003f7280`, `0x003f7410`) store
/// them.
pub const CONSTANTS: u32 = 0x0040_7738;
/// The registers [`CONSTANTS`] holds.
pub const CONSTANT_REGISTERS: usize = 192;

/// The guest's default palette header (`0x00015b70` binds it at every
/// stage).
pub const DEFAULT_PALETTE: u32 = 0x0048_4dbc;

/// What guest render-state slot `slot` holds: D3D's render-state shadow
/// (`engine::render_loop::states::SHADOW`, a dword per guest slot), which
/// the inline `SetRenderState` writers and the setters store. `None` past
/// the build's slots or when it does not read.
pub fn render_state(mem: &dyn GuestMemory, slot: u32) -> Option<u32> {
    if slot as usize >= gfx::xdk::RenderStateTable::halo2().len() {
        return None;
    }
    mem.u32(engine::render_loop::states::SHADOW + 4 * slot).ok()
}

/// The render states `types` from the shadow, decoded as a draw decodes
/// them. `None` when one does not read or holds a value outside its enum.
fn decoded(mem: &dyn GuestMemory, types: &[gfx::state::RenderStateType]) -> Option<gfx::state::RenderState> {
    let table = gfx::xdk::RenderStateTable::halo2();
    let mut rs = gfx::state::RenderState::default();
    for &t in types {
        let slot = table.canonical_to_guest(t.index())?;
        rs.set_raw(t, render_state(mem, slot)?).ok()?;
    }
    Some(rs)
}

/// The stencil state the next draw takes, from the render-state shadow.
pub fn stencil_state(mem: &dyn GuestMemory) -> Option<gfx::state::StencilState> {
    use gfx::state::RenderStateType as R;
    let types = [
        R::StencilEnable,
        R::StencilFunc,
        R::StencilRef,
        R::StencilMask,
        R::StencilWriteMask,
        R::StencilFail,
        R::StencilZFail,
        R::StencilPass,
    ];
    decoded(mem, &types).map(|rs| rs.stencil)
}

/// `D3DRS_CULLMODE` and `D3DRS_FRONTFACE` the next draw takes
/// (`SetRenderState_CullMode` `0x003f53d0` and `_FrontFace` `0x003f5450`
/// keep them in the render-state shadow).
pub fn cull_state(mem: &dyn GuestMemory) -> Option<(gfx::state::CullMode, gfx::state::FrontFace)> {
    use gfx::state::RenderStateType as R;
    decoded(mem, &[R::CullMode, R::FrontFace]).map(|rs| (rs.cull_mode, rs.front_face))
}

/// Constant register `r` (hardware order, register + 96) as bits, from
/// the guest's shadow [`CONSTANTS`]. `None` past the registers or when it
/// does not read.
pub fn constant(mem: &dyn GuestMemory, r: usize) -> Option<[u32; 4]> {
    if r >= CONSTANT_REGISTERS {
        return None;
    }
    let at = CONSTANTS + 16 * r as u32;
    let mut c = [0; 4];
    for (i, w) in c.iter_mut().enumerate() {
        *w = mem.u32(at + 4 * i as u32).ok()?;
    }
    Some(c)
}

/// The header of the palette bound at `stage` now, as the game records
/// it: its per-stage cache `0x00484dc0`
/// (`engine::render_loop::world_lightmap::STAGE_PALETTES`, an index into
/// the 12-byte headers at `PALETTE_HEADERS`, which `0x00015c90` keeps as it
/// binds a lightmap's palette) names one of the `PALETTE_COUNT` headers,
/// or the stage holds the default palette `[DEFAULT_PALETTE]` that
/// `0x00015b70` binds as it clears the cache.
pub fn stage_palette_header(mem: &dyn GuestMemory, stage: usize) -> Option<u32> {
    use engine::render_loop::world_lightmap::{PALETTE_COUNT, PALETTE_HEADERS, STAGE_PALETTES};
    if stage >= 4 {
        return None;
    }
    let cached = mem.i32(STAGE_PALETTES + 4 * stage as u32).ok()?;
    let count = mem.i32(PALETTE_COUNT).ok()?;
    if (0..count).contains(&cached) {
        return Some(PALETTE_HEADERS + 12 * cached as u32);
    }
    mem.u32(DEFAULT_PALETTE).ok().filter(|&h| h != 0)
}

/// The `A8R8G8B8` entries of the palette bound at `stage` now
/// ([`stage_palette_header`]); `None` when none reads.
pub fn stage_palette(mem: &dyn GuestMemory, stage: usize) -> Option<Vec<u32>> {
    palette_entries(mem, stage_palette_header(mem, stage)?)
}

/// The entries of the palette whose `X_D3DPalette` header is at `header`:
/// `Common` (type 3 in bits 16..19, the size code in the top two bits:
/// 256, 128, 64 or 32 entries) and `Data` (the entries, contiguous
/// memory, [`data_address`]). `None` when it does not read or is not a
/// palette.
pub fn palette_entries(mem: &dyn GuestMemory, header: u32) -> Option<Vec<u32>> {
    if header == 0 {
        return None;
    }
    let common = mem.u32(header).ok()?;
    if (common >> 16) & 7 != 3 {
        return None;
    }
    let data = mem.u32(header.wrapping_add(4)).ok()?;
    let entries = 256usize >> (common >> 30);
    let bytes = mem.bytes(data_address(data), 4 * entries).ok()?;
    Some(bytes.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// `D3DDevice_Clear`'s flags and values as a frame clear: target bits
/// `0xf0` (all four channels or none), depth `1`, stencil `2`.
fn clear(flags: u32, colour: u32, z: u32, stencil: u32) -> gfx::Clear {
    let channels = flags & 0xf0;
    if channels != 0 && channels != 0xf0 {
        unimplemented!("targets: a clear of colour channels {channels:#x}");
    }
    let c = |s: u32| ((colour >> s) & 0xff) as f32 / 255.0;
    gfx::Clear {
        color: (channels != 0).then(|| [c(16), c(8), c(0), c(24)]),
        depth: (flags & 1 != 0).then(|| f32::from_bits(z)),
        stencil: (flags & 2 != 0).then_some(stencil as u8),
    }
}

struct Targets {
    /// The bound colour and depth headers, then the viewport the guest
    /// last set (`x, y, width, height, min_z, max_z`), as `bound` reports.
    bound: Mutex<[u32; 8]>,
}

impl Targets {
    fn bound(&self) -> std::sync::MutexGuard<'_, [u32; 8]> {
        self.bound.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Host for Targets {
    fn bind(&self, mem: &mut dyn GuestMemory, bind: Bind, colour: u32, depth: u32) -> bool {
        if bind == Bind::Kept {
            return true;
        }
        let op = FrameOp::Bind { colour: target_ref(&*mem, colour, true), depth: target_ref(&*mem, depth, false) };
        frame_builder().push(op);
        let mut b = self.bound();
        b[0] = colour;
        b[1] = depth;
        true
    }

    fn clear(&self, mem: &mut dyn GuestMemory, flags: u32, colour: u32, z: u32, stencil: u32) -> bool {
        let [c, d, ..] = *self.bound();
        let op = FrameOp::Clear {
            colour: target_ref(&*mem, c, true),
            depth: target_ref(&*mem, d, false),
            clear: clear(flags, colour, z, stencil),
        };
        frame_builder().push(op);
        true
    }

    fn bound(&self) -> Option<[u32; 8]> {
        Some(*self.bound())
    }

    fn texture(&self, _: &mut dyn GuestMemory, _: u32, _: u32) -> bool {
        true
    }

    fn pixel_shader(&self, _: &mut dyn GuestMemory, _: u32) -> bool {
        true
    }

    fn render_state(&self, _: &mut dyn GuestMemory, _: u32, _: u32) -> bool {
        true
    }

    fn simple_state(&self, _: &mut dyn GuestMemory, _: u32, _: u32) -> bool {
        true
    }

    fn stored_state(&self, mem: &dyn GuestMemory, slot: u32) -> Option<u32> {
        render_state(mem, slot)
    }

    fn constants(&self, _: &mut dyn GuestMemory, _: u32, _: u32, _: u32) -> bool {
        true
    }

    fn registers(&self, _: &mut dyn GuestMemory, _: u32, _: u32, _: u32) -> bool {
        true
    }

    fn texture_stage_state(&self, _: &mut dyn GuestMemory, _: u32, _: u32, _: u32) -> bool {
        true
    }

    fn vertex_program(&self, _: &mut dyn GuestMemory, _: u32, _: u32) -> bool {
        true
    }

    fn vertex_input(&self, _: &mut dyn GuestMemory, _: u32, _: u32, _: u32) -> bool {
        true
    }

    fn select_vertex_program(&self, _: &mut dyn GuestMemory, _: u32, _: u32) -> bool {
        true
    }

    fn constant(&self, mem: &dyn GuestMemory, r: usize) -> Option<[u32; 4]> {
        constant(mem, r)
    }

    /// Reached only when the put is at the push buffer's limit: nothing
    /// here advances it ([`Self::push_viewport`]), so a guest writer
    /// filled it, and with no device nothing drains it.
    fn make_space(&self, mem: &mut dyn GuestMemory, _: u32, _: u32) -> Option<u32> {
        let device = mem.u32(render_bridge::native_targets::DEVICE).ok();
        unimplemented!("targets: push buffer full at device {device:x?}: a guest writer filled it and nothing drains it")
    }

    /// The viewport fragment's methods have no reader: the put stays.
    fn push_viewport(&self, _: &mut dyn GuestMemory, put: u32, _: u32) -> Option<u32> {
        Some(put)
    }

    fn surface_size(&self, mem: &dyn GuestMemory, header: u32) -> Option<(u32, u32)> {
        // `X_D3DPixelContainer`: `Format` at +0x0c, `Size` at +0x10.
        let format = mem.u32(header.wrapping_add(0x0c)).ok()?;
        let size = mem.u32(header.wrapping_add(0x10)).ok()?;
        gfx::texture::TextureDesc::from_header(format, size).ok().map(|d| (d.width, d.height))
    }

    fn viewport(&self, mem: &mut dyn GuestMemory, at: u32) -> bool {
        let mut viewport = [0; 6];
        for (i, w) in viewport.iter_mut().enumerate() {
            let Ok(v) = mem.u32(at + 4 * i as u32) else { return false };
            *w = v;
        }
        self.bound()[2..].copy_from_slice(&viewport);
        true
    }

    fn copy_view(&self, mem: &mut dyn GuestMemory, target: u32) -> bool {
        let Some(header) = crate::native_shadows_glue::target_header(&*mem, target) else { return false };
        let from = target_ref(&*mem, self.bound()[0], true);
        frame_builder().push(FrameOp::Copy { from, to: target_ref(&*mem, header, true) });
        true
    }

    fn restore(&self, _: &mut dyn GuestMemory, _: &[render_bridge::open_shader::Write]) -> bool {
        true
    }
}

/// The frame tail's overlay, placed through the overlay's stage.
struct Tail;

impl render_bridge::frame_tail::Host for Tail {
    fn place(&self, mem: &mut dyn GuestMemory, overlay: frame_scene::Overlay) -> bool {
        crate::hud_overlay::place_tail(mem, overlay)
    }
}

pub fn install() {
    native_targets::set_host(Arc::new(Targets { bound: Mutex::new([0; 8]) }));
    render_bridge::frame_tail::set_host(Arc::new(Tail));
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::render_loop::states::SHADOW;
    use engine::render_loop::world_lightmap::{PALETTE_COUNT, PALETTE_HEADERS, STAGE_PALETTES};
    use engine::testing::FakeGuest;
    use gfx::state::{CompareFunc, CullMode, FrontFace, RenderStateType as R, StencilOp};

    /// The guest slot of canonical state `t`.
    fn slot(t: R) -> u32 {
        gfx::xdk::RenderStateTable::halo2().canonical_to_guest(t.index()).unwrap()
    }

    fn shadow(m: &mut FakeGuest, t: R, value: u32) {
        m.put_u32(SHADOW + 4 * slot(t), value);
    }

    /// The shadow with every state the readers take at its default.
    fn states() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.map(SHADOW, 4 * gfx::xdk::RenderStateTable::halo2().len() as u32);
        for (t, v) in [
            (R::StencilFunc, 0x207),
            (R::StencilFail, 0x1e00),
            (R::StencilZFail, 0x1e00),
            (R::StencilPass, 0x1e00),
            (R::FrontFace, 0x900),
        ] {
            shadow(&mut m, t, v);
        }
        m
    }

    #[test]
    fn a_render_state_is_its_shadow_word_and_none_past_the_slots() {
        let mut m = states();
        m.put_u32(SHADOW + 4 * 7, 0x1234);
        assert_eq!(render_state(&m, 7), Some(0x1234));
        let past = gfx::xdk::RenderStateTable::halo2().len() as u32;
        m.put_u32(SHADOW + 4 * past, 1);
        assert_eq!(render_state(&m, past), None);
    }

    #[test]
    fn the_stencil_decodes_the_shadows_stencil_states() {
        let mut m = states();
        shadow(&mut m, R::StencilEnable, 1);
        shadow(&mut m, R::StencilFunc, 0x202);
        shadow(&mut m, R::StencilRef, 0xff);
        shadow(&mut m, R::StencilMask, 0x0f);
        shadow(&mut m, R::StencilWriteMask, 0xf0);
        shadow(&mut m, R::StencilPass, 0x1e01);
        let s = stencil_state(&m).unwrap();
        assert!(s.enable);
        assert_eq!(s.func, CompareFunc::Equal);
        assert_eq!((s.reference, s.read_mask, s.write_mask), (0xff, 0x0f, 0xf0));
        assert_eq!((s.fail, s.depth_fail, s.pass), (StencilOp::Keep, StencilOp::Keep, StencilOp::Replace));
    }

    #[test]
    fn a_stencil_word_outside_its_enum_does_not_read() {
        let mut m = states();
        shadow(&mut m, R::StencilFunc, 0x9999);
        assert_eq!(stencil_state(&m), None);
        assert_eq!(stencil_state(&FakeGuest::new()), None, "an unmapped shadow");
    }

    #[test]
    fn the_cull_is_the_shadows_cullmode_and_frontface() {
        let mut m = states();
        shadow(&mut m, R::CullMode, 0x901);
        shadow(&mut m, R::FrontFace, 0x900);
        assert_eq!(cull_state(&m), Some((CullMode::Ccw, FrontFace::Cw)));
        shadow(&mut m, R::CullMode, 7);
        assert_eq!(cull_state(&m), None);
    }

    #[test]
    fn a_constant_register_is_its_sixteen_shadow_bytes() {
        let mut m = FakeGuest::new();
        let r = 5 + 96;
        for i in 0..4 {
            m.put_u32(CONSTANTS + 16 * r as u32 + 4 * i, 0x10 + i);
        }
        assert_eq!(constant(&m, r), Some([0x10, 0x11, 0x12, 0x13]));
        assert_eq!(constant(&m, CONSTANT_REGISTERS), None);
        assert_eq!(constant(&FakeGuest::new(), r), None, "unmapped");
    }

    /// A palette header at `header` of `entries` (a size code's count)
    /// over data at physical `data`, entry `i` being `base + i`.
    fn palette(m: &mut FakeGuest, header: u32, code: u32, data: u32, base: u32) {
        m.put_u32(header, code << 30 | 3 << 16 | 1);
        m.put_u32(header + 4, data);
        for i in 0..(256 >> code) {
            m.put_u32(data_address(data) + 4 * i, base + i);
        }
    }

    #[test]
    fn palette_entries_read_the_headers_count_from_contiguous_memory() {
        let mut m = FakeGuest::new();
        palette(&mut m, 0x0050_0000, 0, 0x0123_0000, 0xff00_0000);
        let p = palette_entries(&m, 0x0050_0000).unwrap();
        assert_eq!(p.len(), 256);
        assert_eq!((p[0], p[255]), (0xff00_0000, 0xff00_00ff));
        palette(&mut m, 0x0050_0100, 2, 0x0124_0000, 7);
        assert_eq!(palette_entries(&m, 0x0050_0100).map(|p| p.len()), Some(64));
    }

    #[test]
    fn a_header_that_is_not_a_palette_has_no_entries() {
        let mut m = FakeGuest::new();
        palette(&mut m, 0x0050_0000, 0, 0x0123_0000, 0);
        m.put_u32(0x0050_0000, 4 << 16);
        assert_eq!(palette_entries(&m, 0x0050_0000), None, "a texture");
        assert_eq!(palette_entries(&m, 0), None);
    }

    /// The game's palette globals: the default header, no cached stage,
    /// `count` headers.
    fn palettes(count: i32) -> FakeGuest {
        let mut m = FakeGuest::new();
        palette(&mut m, 0x0050_0000, 0, 0x0123_0000, 0xaa00_0000);
        m.put_u32(DEFAULT_PALETTE, 0x0050_0000);
        for s in 0..4 {
            m.put_u32(STAGE_PALETTES + 4 * s, u32::MAX);
        }
        m.put_u32(PALETTE_COUNT, count as u32);
        m
    }

    #[test]
    fn a_stage_with_no_cached_palette_holds_the_default() {
        let m = palettes(2);
        for s in 0..4 {
            assert_eq!(stage_palette_header(&m, s), Some(0x0050_0000));
            assert_eq!(stage_palette(&m, s).map(|p| p[1]), Some(0xaa00_0001));
        }
        assert_eq!(stage_palette_header(&m, 4), None);
    }

    #[test]
    fn a_cached_stage_index_names_its_palette_header() {
        let mut m = palettes(2);
        m.put_u32(STAGE_PALETTES + 12, 1);
        palette(&mut m, PALETTE_HEADERS + 12, 0, 0x0125_0000, 0xbb00_0000);
        assert_eq!(stage_palette_header(&m, 3), Some(PALETTE_HEADERS + 12));
        assert_eq!(stage_palette(&m, 3).map(|p| p[0]), Some(0xbb00_0000));
        assert_eq!(stage_palette_header(&m, 0), Some(0x0050_0000), "the other stages keep the default");
        // An index past the headers is no cached palette.
        m.put_u32(STAGE_PALETTES + 12, 2);
        assert_eq!(stage_palette_header(&m, 3), Some(0x0050_0000));
    }

    #[test]
    fn the_viewport_push_writes_nothing_and_keeps_the_put() {
        let t = Targets { bound: Mutex::new([0; 8]) };
        let mut m = FakeGuest::new();
        assert_eq!(t.push_viewport(&mut m, 0x1000, 0x0040_4fe0), Some(0x1000));
    }

    #[test]
    fn a_viewport_that_does_not_read_is_refused_and_leaves_the_bound_one() {
        let t = Targets { bound: Mutex::new([1, 2, 3, 4, 5, 6, 7, 8]) };
        let mut m = FakeGuest::new();
        assert!(!t.viewport(&mut m, 0x2000));
        assert_eq!(*t.bound(), [1, 2, 3, 4, 5, 6, 7, 8]);
        for i in 0..6 {
            m.put_u32(0x2000 + 4 * i, 10 + i);
        }
        assert!(t.viewport(&mut m, 0x2000));
        assert_eq!(*t.bound(), [1, 2, 10, 11, 12, 13, 14, 15]);
    }
}
