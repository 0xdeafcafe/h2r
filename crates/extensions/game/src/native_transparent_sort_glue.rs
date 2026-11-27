//! The GPU back-to-front sort of the transparent list's keys: registers
//! `native_render::transparent_sort::GpuSort` on the game's upload device
//! as `render_bridge::native_scene_lists::set_gpu_sorter`
//! (docs/graphics/native-transparents.md, "The sort on the GPU").

use std::sync::Arc;

use native_render::transparent_sort::{GpuSort, Keys};

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let enabled = hooks::Switches::from_env().is_ok_and(|s| hooks::switches::native_render_on(&s));
    if !enabled {
        render_bridge::native_scene_lists::set_gpu_sorter(None);
        return;
    }
    let Some((gpu_device, queue)) = device.lock().ok().and_then(|d| d.backend.upload_device()) else {
        render_bridge::native_scene_lists::set_gpu_sorter(None);
        return;
    };
    let sort = GpuSort::new(&gpu_device);
    render_bridge::native_scene_lists::set_gpu_sorter(Some(Arc::new(move |keys: &Keys| {
        Some(sort.sort(&gpu_device, &queue, keys))
    })));
}
