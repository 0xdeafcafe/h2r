//! The host of `render_bridge::native_targets`: the scene's target select
//! and clear and the frame's finish made on the shared device without
//! their guest D3D entries (docs/graphics/framescene.md, "Slice 6";
//! docs/graphics/retire-d3d-display.md, "The frame step"), the view setup's
//! and view end's device calls ("The view setup and view end"), and the frame
//! tail's overlay placed on it ("The frame tail").

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use render_bridge::native_targets::{self, Bind, Host};

use crate::native_lights_glue::DeviceMemory;

struct Targets<B: d3d::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    logged: Mutex<bool>,
}

impl<B: d3d::Backend + Send + 'static> Targets<B> {
    fn refused(&self, what: &str, e: d3d::D3dError) -> bool {
        let mut logged = self.logged.lock().unwrap_or_else(|e| e.into_inner());
        if !std::mem::replace(&mut *logged, true) {
            eprintln!("native-targets: the device refused {what}: {e} (logged once)");
        }
        false
    }
}

impl<B: d3d::Backend + Send + 'static> Host for Targets<B> {
    fn bind(&self, mem: &mut dyn GuestMemory, bind: Bind, colour: u32, depth: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let mut m = DeviceMemory(mem);
        let made = match bind {
            Bind::Kept => return true,
            Bind::SameSize => device.set_render_target_same_size(&mut m, colour, depth),
            Bind::Full => device.set_render_target(&mut m, colour, depth),
        };
        drop(device);
        made.map_or_else(|e| self.refused("a bind", e), |()| true)
    }

    fn clear(&self, mem: &mut dyn GuestMemory, flags: u32, colour: u32, z: u32, stencil: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.clear(&mut DeviceMemory(mem), 0, 0, flags, colour, f32::from_bits(z), stencil);
        drop(device);
        made.map_or_else(|e| self.refused("a clear", e), |()| true)
    }

    fn bound(&self) -> Option<[u32; 8]> {
        self.device.lock().ok().map(|d| d.bound_target())
    }

    fn texture(&self, mem: &mut dyn GuestMemory, stage: u32, texture: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.set_texture(&mut DeviceMemory(mem), stage, texture);
        drop(device);
        made.map_or_else(|e| self.refused("a texture", e), |()| true)
    }

    fn pixel_shader(&self, mem: &mut dyn GuestMemory, handle: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.set_pixel_shader(&mut DeviceMemory(mem), handle);
        drop(device);
        made.map_or_else(|e| self.refused("a pixel shader", e), |()| true)
    }

    fn render_state(&self, mem: &mut dyn GuestMemory, slot: u32, value: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.set_render_state(&mut DeviceMemory(mem), slot, value);
        drop(device);
        made.map_or_else(|e| self.refused("a render state", e), |()| true)
    }

    fn simple_state(&self, mem: &mut dyn GuestMemory, header: u32, value: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.set_render_state_simple(&mut DeviceMemory(mem), header, value);
        drop(device);
        made.map_or_else(|e| self.refused("a simple render state", e), |()| true)
    }

    fn stored_state(&self, mem: &dyn GuestMemory, slot: u32) -> Option<u32> {
        let read = crate::native_world_glue::ReadMemory(mem);
        self.device.lock().ok()?.render_state(&read, slot)
    }

    fn constants(&self, mem: &mut dyn GuestMemory, register: u32, data: u32, dwords: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.set_vertex_shader_constant_dwords(&mut DeviceMemory(mem), register, data, dwords);
        drop(device);
        made.map_or_else(|e| self.refused("the constants", e), |()| true)
    }

    fn registers(&self, mem: &mut dyn GuestMemory, register: u32, data: u32, count: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.set_vertex_shader_constant(&mut DeviceMemory(mem), register, data, count, true);
        drop(device);
        made.map_or_else(|e| self.refused("the constant registers", e), |()| true)
    }

    fn texture_stage_state(&self, mem: &mut dyn GuestMemory, stage: u32, state: u32, value: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.set_texture_stage_state(&mut DeviceMemory(mem), stage, state, value);
        drop(device);
        made.map_or_else(|e| self.refused("a texture stage state", e), |()| true)
    }

    fn vertex_program(&self, mem: &mut dyn GuestMemory, function: u32, address: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.load_vertex_shader_program(&mut DeviceMemory(mem), function, address);
        drop(device);
        made.map_or_else(|e| self.refused("a vertex program", e), |()| true)
    }

    fn vertex_input(&self, mem: &mut dyn GuestMemory, handle: u32, count: u32, inputs: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.set_vertex_shader_input(&mut DeviceMemory(mem), handle, count, inputs);
        drop(device);
        made.map_or_else(|e| self.refused("the vertex inputs", e), |()| true)
    }

    fn select_vertex_program(&self, mem: &mut dyn GuestMemory, vaf: u32, address: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.select_vertex_shader_direct(&mut DeviceMemory(mem), vaf, address);
        drop(device);
        made.map_or_else(|e| self.refused("a vertex program select", e), |()| true)
    }

    fn constant(&self, r: usize) -> Option<[u32; 4]> {
        // 192 registers (`d3d::Device::constant`'s hardware order).
        (r < 192).then(|| self.device.lock().ok().map(|d| d.constant(r).map(f32::to_bits))).flatten()
    }

    fn make_space(&self, mem: &mut dyn GuestMemory, minimum: u32, requested: u32) -> Option<u32> {
        let Ok(mut device) = self.device.lock() else { return None };
        let made = device.make_requested_space(&mut DeviceMemory(mem), minimum, requested);
        drop(device);
        made.map_err(|e| self.refused("room in the push buffer", e)).ok()
    }

    fn push_viewport(&self, mem: &mut dyn GuestMemory, put: u32, at: u32) -> Option<u32> {
        let Ok(mut device) = self.device.lock() else { return None };
        let made = device.push_viewport(&mut DeviceMemory(mem), put, at);
        drop(device);
        made.map_err(|e| self.refused("the viewport push", e)).ok()
    }

    fn surface_size(&self, mem: &dyn GuestMemory, header: u32) -> Option<(u32, u32)> {
        self.device.lock().ok()?.surface_size(&crate::native_world_glue::ReadMemory(mem), header)
    }

    fn viewport(&self, mem: &mut dyn GuestMemory, at: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.set_viewport(&mut DeviceMemory(mem), at);
        drop(device);
        made.map_or_else(|e| self.refused("a viewport", e), |()| true)
    }

    fn copy_view(&self, mem: &mut dyn GuestMemory, target: u32) -> bool {
        let Some(header) = crate::native_shadows_glue::target_header(&*mem, target) else { return false };
        let Ok(mut device) = self.device.lock() else { return false };
        if !device.can_external_pass() {
            return false;
        }
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let plan = d3d::ExternalPlan {
            stage: timeline::native::Stage::Water,
            view,
            ranges: 1,
            slots: vec![Some(d3d::ExternalSlot { header, written: true })],
        };
        match device.prepare_external_pass(&mut DeviceMemory(mem), plan, Box::new(ViewCopy)) {
            Ok(Some(reservation)) => {
                reservation.commit();
                true
            }
            Ok(None) => false,
            Err(e) => self.refused("a view copy", e),
        }
    }

    fn restore(&self, mem: &mut dyn GuestMemory, writes: &[render_bridge::open_shader::Write]) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        crate::native_layers_glue::restore_list(&mut device, mem, writes).is_ok()
    }
}

/// The bound colour copied into the target of the plan's one slot, in
/// order with the draws after it (`snapshot_gpu::Snapshots::into_target`):
/// `0x0003bea0`'s copy of the water end's ocean
/// (`render_bridge::water_end::ocean`, docs/graphics/retire-d3d-display.md,
/// "The water end").
struct ViewCopy;

impl ViewCopy {
    fn snapshot(
        color: &wgpu::TextureView, slot: Option<&d3d::backend::ExternalColor>,
    ) -> Result<native_render::snapshot_gpu::ViewSnapshot, &'static str> {
        let slot = slot.ok_or("the refraction target unavailable")?;
        if !color.texture().usage().contains(wgpu::TextureUsages::TEXTURE_BINDING) {
            return Err("the bound colour cannot be sampled for a snapshot");
        }
        Ok(native_render::snapshot_gpu::Snapshots::default().into_target(color, &slot.attachment, slot.format))
    }
}

impl d3d::ExternalPass for ViewCopy {
    fn diagnostic_name(&self) -> &'static str {
        "water ocean view copy"
    }

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        match Self::snapshot(t.color, t.slots.first().and_then(Option::as_ref)) {
            Ok(copy) => {
                copy.record(t.device, t.encoder);
                1
            }
            Err(_) => 0,
        }
    }

    fn record_encoded(
        &mut self, t: &d3d::backend::ExternalAttachments, _request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        Some(Self::snapshot(&t.color, t.slots.first().and_then(Option::as_ref)).map(|copy| {
            let device = t.device.clone();
            d3d::backend::ExternalEncoded {
                ranges: 1,
                draws: 1,
                passes: 1,
                commands: Box::new(move |encoder: &mut wgpu::CommandEncoder| copy.record(&device, encoder)),
            }
        }))
    }
}

/// The frame tail's overlay, placed through the overlay's pass.
struct Tail<B: d3d::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

impl<B: d3d::Backend + Send + 'static> render_bridge::frame_tail::Host for Tail<B> {
    fn place(&self, mem: &mut dyn GuestMemory, overlay: frame_scene::Overlay) -> bool {
        crate::hud_overlay::place_tail(&self.device, mem, overlay)
    }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| native_targets::on(&s)) {
        return;
    }
    native_targets::set_host(Arc::new(Targets { device: Arc::clone(device), logged: Mutex::new(false) }));
    render_bridge::frame_tail::set_host(Arc::new(Tail { device: Arc::clone(device) }));
}
