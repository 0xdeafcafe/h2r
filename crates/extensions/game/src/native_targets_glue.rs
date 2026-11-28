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
/// buffer (`colour`) or depth. A game role has no frame target yet.
pub fn target_ref(mem: &dyn GuestMemory, header: u32, colour: bool) -> TargetRef {
    if header == 0 {
        return TargetRef::None;
    }
    match native_targets::role(header, &|a| mem.u32(a).ok()) {
        None if colour => TargetRef::SceneColour,
        None => TargetRef::SceneDepth,
        Some(role) => unimplemented!("targets: frame target for {role:?}"),
    }
}

/// The guest virtual address of a resource's `Data` word: the Xbox
/// contiguous mapping, `0x80000000 | (data & 0x0FFFFFFF)`.
pub fn data_address(data: u32) -> u32 {
    0x8000_0000 | (data & 0x0FFF_FFFF)
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
        self.device.lock().ok()?.surface_size(&crate::native_world_glue::ReadMemory(mem), header)
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
