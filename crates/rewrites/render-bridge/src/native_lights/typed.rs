//! Whole-pass admission for the first-person lights
//! ([`super::first_person`]) once their item lists are made, and the
//! read-only mirror the cut light frame plans its groups over. Only the
//! committed pass puts groups in the scene or replaces guest constants and
//! tail state.

use engine::memory::{GuestMemory, MemFault};
use engine::render::light_gather::{Refusal, Writes, typed, typed_tail};
use native_render::{light_graph::Pass, light_passes};

use super::{HOST, SceneGroup, TypedPasses, VIEW_TARGET};

/// The bounds helper starts with an empty, exception-free x87 stack.
pub(super) fn fpu_ready(tag: u16, status: u16, control: u16) -> bool {
    tag == u16::MAX && status & 0x80 == 0 && status & !control & 0x3f == 0
}

/// Record the same stores the guest-memory writers make. The device
/// commits them with its own journal after every copy has reserved its draws.
pub(super) struct Mirror<'a> {
    pub(super) mem: &'a dyn GuestMemory,
    pub(super) writes: Writes,
}

impl GuestMemory for Mirror<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        buf.copy_from_slice(&self.writes.read(self.mem, addr, buf.len())?);
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        self.writes.array(addr, data.to_vec());
        Ok(())
    }
}

/// Whether a renderer is attached to take a typed pass. Without one every
/// plan below would be read and then thrown away.
pub(super) fn host_ready() -> bool {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

pub(super) fn submit(
    mem: &mut dyn GuestMemory,
    graph: Pass<'_>,
    groups: &[SceneGroup],
    globals: &typed::Globals,
    tail: &typed_tail::Tail,
    draw_byte: Option<(u32, u8)>,
) -> bool {
    let mut target = [0; 8];
    target[..2].copy_from_slice(&tail.surfaces);
    target[2..].copy_from_slice(&tail.viewport);
    if VIEW_TARGET.get() != Some(target) {
        return false;
    }
    let Ok(view) = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX) else {
        return false;
    };
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return false;
    };
    let mut mirror = Mirror {
        mem,
        writes: Writes::new(),
    };
    if globals.write(&mut mirror).is_err() || tail.write(&mut mirror).is_err() {
        return false;
    }
    if let Some((addr, value)) = draw_byte {
        mirror.writes.byte(addr, value);
    }
    let writes = mirror.writes;
    host.draw_typed(
        mem,
        &TypedPasses {
            graph,
            groups,
            view_target: target,
            clip: tail.clip,
            clip_header: tail.clip_header,
            writes: &writes,
            view,
        },
    )
}

/// Layer 14 always tests the receiver mask, and first-person lights also
/// always test layer 11. Skip the sphere projection and material walk when
/// one of these draws or the shader gel already needs the ordinary passes.
pub(super) fn check_view_layers(
    mem: &dyn GuestMemory,
    definition: u32,
    first_person: bool,
) -> Result<(), Refusal> {
    let mask = mem.u32(light_passes::LIGHT_LIST + 8)?;
    if mask & (1 << 14) != 0
        || (first_person && mask & (1 << 11) != 0)
        || mem.u32(definition.wrapping_add(engine::light_passes::DEFINITION_GEL_SHADER))?
            != u32::MAX
    {
        return Err(Refusal::Unsupported);
    }
    Ok(())
}
