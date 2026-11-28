//! The host of `render_bridge::native_flare_query`: the flares' count
//! pass as a stage where the guest's walker would draw its quads
//! (docs/graphics/d3d-visibility.md, "The query in Rust"). There is no
//! device, so a slot the guest ended holds no device count.

use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;
use frame_types::Slot;
use native_render::flare_gpu::{DepthTest, FlareQueryGpu, QueryReadback};
use render_bridge::native_flare_query::{self, Counts, DeviceResult, Host, Quad};

/// One pass's counts: recorded on the render thread, read on the game's.
#[derive(Default)]
struct PassCounts(Mutex<Recording>);

#[derive(Default)]
enum Recording {
    #[default]
    Waiting,
    Recorded(QueryReadback),
    /// The pass did not record: its counts never come, and the bytes keep
    /// their values.
    Refused,
}

impl Counts for PassCounts {
    fn read(&self) -> Option<Arc<[u32]>> {
        match &*self.0.lock().unwrap_or_else(|e| e.into_inner()) {
            Recording::Recorded(r) => r.try_read(),
            Recording::Waiting | Recording::Refused => None,
        }
    }
}

/// The pass's pipeline on the device last recorded with.
static GPU: Mutex<Option<(wgpu::Device, FlareQueryGpu)>> = Mutex::new(None);

struct FlareQueryPass {
    quads: Arc<Vec<Quad>>,
    counts: Arc<PassCounts>,
}

impl native_render::Stage for FlareQueryPass {
    fn name(&self) -> &'static str {
        "flare query"
    }

    /// The walker's quads are W-buffered (`ZEnable(2)`) under the view's
    /// W clip. The last recording's readback is the one kept.
    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &native_render::program::ViewInputs) -> native_render::StageWork {
        let test = match view.w_clip {
            Some((range, clamp)) => DepthTest { w_buffered: true, range, clamp },
            None => DepthTest { w_buffered: true, range: [0.0, f32::MAX], clamp: true },
        };
        let (quads, counts, guest, z_scale) = (Arc::clone(&self.quads), Arc::clone(&self.counts), view.guest_size, view.z_scale);
        let (device, depth) = (t.device.clone(), t.depth.clone());
        crate::native_targets_glue::encoded(move |encoder| {
            let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
            if gpu.as_ref().is_none_or(|(d, _)| *d != device) {
                *gpu = Some((device.clone(), FlareQueryGpu::new(&device)));
            }
            let Some((_, pass)) = gpu.as_ref() else { return };
            let recorded = match pass.record(&device, encoder, depth.texture(), guest, test, z_scale, &quads) {
                Ok(r) => Recording::Recorded(r),
                Err(why) => {
                    native_flare_query::QUERY.dropped(why_cause(why));
                    Recording::Refused
                }
            };
            *counts.0.lock().unwrap_or_else(|e| e.into_inner()) = recorded;
        })
    }
}

fn why_cause(why: &str) -> &'static str {
    if why.contains("depth") { "depth" } else { "empty" }
}

struct Query;

impl Host for Query {
    fn query(&self, mem: &mut dyn GuestMemory, quads: Vec<Quad>) -> Option<Arc<dyn Counts>> {
        let counts = Arc::new(PassCounts::default());
        let pass = FlareQueryPass { quads: Arc::new(quads), counts: Arc::clone(&counts) };
        crate::native_targets_glue::push(&*mem, Slot::Transparents, pass);
        Some(counts)
    }

    fn device_result(&self, _: &mut dyn GuestMemory, slot: u32) -> DeviceResult {
        unimplemented!("flare query: device visibility result for slot {slot}")
    }
}

pub fn install() {
    native_flare_query::set_host(Arc::new(Query));
}
