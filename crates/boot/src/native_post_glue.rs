//! The `native-post` host (docs/graphics/native-renderer.md, stage 5):
//! the device side of a skipped `render_screen_effects`.
//!
//! Inert unless `impl.render.native_post` is on: the hook is not placed
//! otherwise. It takes no call yet (`accepts` answers no), so every call
//! keeps its D3D passes and the hook only counts them. What a call it
//! takes needs is here already:
//!
//! - **The prologue's render states**: the restore list of the shader
//!   draw-state reset, set through `d3d::Device::set_render_states_inline`
//!   over the hook's guest memory, as `0x00018900` sets them (d3d's
//!   `inline_states` test runs the two side by side);
//! - **The pass**: none yet; `open` asks for nothing. The colour pass's
//!   numbers are `engine::colour_pass` (stage 5, item 3).

use std::sync::Arc;

use engine::memory::GuestMemory;
use feature::native_post::Host;
use native_render::post::PostFrame;

use crate::native_lights_glue::DeviceMemory;

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn accepts(&self, _mem: &dyn GuestMemory, _frame: &PostFrame) -> bool {
        false
    }

    fn prologue(&self, mem: &mut dyn GuestMemory, states: &[(u32, u32)]) -> bool {
        let Ok(mut d) = self.device.lock() else { return false };
        d.set_render_states_inline(&mut DeviceMemory(mem), states).unwrap_or(false)
    }

    fn open(&self, _frame: &PostFrame) -> bool {
        false
    }
}

/// Installs the host over `device`.
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    feature::native_post::set_host(Arc::new(Glue {
        device: Arc::clone(device),
    }));
}
