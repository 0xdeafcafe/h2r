//! The host of `render_bridge::native_targets`: the scene's target select,
//! clear and view copy as frame ops (docs/graphics/native-renderer.md,
//! "How a frame flows"), and the frame tail's overlay. There is no device:
//! a guest state write has no reader and is taken as made, a read of
//! device state panics by name.
//!
//! The stage glue's shared steps live here too: [`push`] places a typed
//! stage at its slot, [`encoded`] wraps encoder commands as stage work and
//! [`target_ref`] names a guest surface header as a frame target.

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use frame_types::{GameTarget, Slot, SlotAt, TargetRef};
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
/// buffer (`colour`) or depth; a game role is the frame graph's surface
/// for it at the header's size (`TargetRef::Game`). A role whose header
/// has no size panics by name.
pub fn target_ref(mem: &dyn GuestMemory, header: u32, colour: bool) -> TargetRef {
    if header == 0 {
        return TargetRef::None;
    }
    match native_targets::role(header, &|a| mem.u32(a).ok()) {
        None if colour => TargetRef::SceneColour,
        None => TargetRef::SceneDepth,
        Some(role) => {
            let size = header_size(mem, header).unwrap_or_else(|| unimplemented!("targets: {role:?}'s surface has no size"));
            TargetRef::Game(GameTarget { role, size, depth: !colour })
        }
    }
}

/// The size of the surface whose `X_D3DPixelContainer` is at `header`:
/// `Format` at `+0x0c`, `Size` at `+0x10`.
pub fn header_size(mem: &dyn GuestMemory, header: u32) -> Option<(u32, u32)> {
    let format = mem.u32(header.wrapping_add(0x0c)).ok()?;
    let size = mem.u32(header.wrapping_add(0x10)).ok()?;
    gfx::texture::TextureDesc::from_header(format, size).ok().map(|d| (d.width, d.height))
}

/// The guest virtual address of a resource's `Data` word: the Xbox
/// contiguous mapping, `0x80000000 | (data & 0x0FFFFFFF)`.
pub fn data_address(data: u32) -> u32 {
    0x8000_0000 | (data & 0x0FFF_FFFF)
}

/// `D3DDevice_Clear`'s flags and values as a frame clear: target bits
/// `0xf0` (all four channels; some of them is [`masked`]'s), depth `1`,
/// stencil `2`.
fn clear(flags: u32, colour: u32, z: u32, stencil: u32) -> gfx::Clear {
    let channels = flags & 0xf0;
    gfx::Clear {
        color: (channels == 0xf0).then(|| argb(colour)),
        depth: (flags & 1 != 0).then(|| f32::from_bits(z)),
        stencil: (flags & 2 != 0).then_some(stencil as u8),
    }
}

/// A D3D ARGB colour as RGBA.
fn argb(colour: u32) -> [f32; 4] {
    let c = |s: u32| ((colour >> s) & 0xff) as f32 / 255.0;
    [c(16), c(8), c(0), c(24)]
}

/// The clear of some colour channels (`0x10` red to `0x80` alpha, not
/// all four) `flags` asks, as the stage that makes it.
fn masked(flags: u32, colour: u32) -> Option<native_render::masked_clear::MaskedClear> {
    let channels = flags & 0xf0;
    (channels != 0 && channels != 0xf0)
        .then(|| native_render::masked_clear::MaskedClear { channels: native_render::masked_clear::channels(channels), colour: argb(colour) })
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
        // Some of the channels: a stage on the bound colour, outside the
        // view's slots (it reads no view).
        if let Some(stage) = masked(flags, colour)
            && c != 0
        {
            push(&*mem, Slot::Outside, stage);
        }
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

    fn stored_state(&self, _: &dyn GuestMemory, slot: u32) -> Option<u32> {
        unimplemented!("targets: render state {slot} from the device")
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

    fn constant(&self, r: usize) -> Option<[u32; 4]> {
        unimplemented!("targets: constant register {r} from the device")
    }

    fn make_space(&self, _: &mut dyn GuestMemory, _: u32, _: u32) -> Option<u32> {
        unimplemented!("targets: push buffer space")
    }

    fn push_viewport(&self, _: &mut dyn GuestMemory, _: u32, _: u32) -> Option<u32> {
        unimplemented!("targets: viewport push fragment")
    }

    fn surface_size(&self, mem: &dyn GuestMemory, header: u32) -> Option<(u32, u32)> {
        header_size(mem, header)
    }

    fn viewport(&self, mem: &mut dyn GuestMemory, at: u32) -> bool {
        let mut b = self.bound();
        for i in 0..6 {
            b[2 + i] = mem.u32(at + 4 * i as u32).unwrap_or_else(|_| unimplemented!("targets: viewport at {at:#x} unread"));
        }
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
    use engine::memory::FlatMemory;
    use frame_types::TargetRole;
    use render_bridge::native_targets::{TARGET_STRIDE, TARGET_SURFACE, TARGETS};

    /// A linear A8R8G8B8 surface of 512 x 256 at `header`.
    fn surface(m: &mut FlatMemory, header: u32) {
        m.set_u32(header + 0x0c, (0x12 << 8) | (2 << 4)).unwrap();
        m.set_u32(header + 0x10, 511 | (255 << 12) | (31 << 24)).unwrap();
    }

    #[test]
    fn a_game_targets_header_is_its_role_at_its_size() {
        let mut m = FlatMemory::new(TARGETS, (0x20 * TARGET_STRIDE) as usize);
        let header = TARGETS + 0x15 * TARGET_STRIDE + TARGET_SURFACE;
        surface(&mut m, header);
        let game = |depth| TargetRef::Game(GameTarget { role: TargetRole::Game(0x15), size: (512, 256), depth });
        assert_eq!(target_ref(&m, header, true), game(false));
        assert_eq!(target_ref(&m, header, false), game(true));
        assert_eq!(target_ref(&m, 0, true), TargetRef::None);
    }

    #[test]
    fn a_clear_of_some_channels_is_the_masked_stage() {
        let alpha = masked(0x80, 0x00ff_ffff).unwrap();
        assert_eq!(alpha.channels, wgpu::ColorWrites::ALPHA);
        assert_eq!(alpha.colour, [1.0, 1.0, 1.0, 0.0]);
        assert!(masked(0xf0, 0).is_none() && masked(0x03, 0).is_none());
        assert_eq!(clear(0x81, 0, 0, 0).color, None, "the load clears no colour");
        assert_eq!(clear(0xf0, 0xff00_0000, 0, 0).color, Some([0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    #[should_panic(expected = "targets: Game(22)'s surface has no size")]
    fn a_game_target_with_no_size_is_named() {
        let mut m = FlatMemory::new(TARGETS, (0x20 * TARGET_STRIDE) as usize);
        let header = TARGETS + 0x16 * TARGET_STRIDE + TARGET_SURFACE;
        // A format byte no D3D format has.
        m.set_u32(header + 0x0c, 0x0a << 8).unwrap();
        target_ref(&m, header, true);
    }
}
