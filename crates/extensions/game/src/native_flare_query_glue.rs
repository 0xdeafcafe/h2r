//! The host of `render_bridge::native_flare_query`: the flares' count
//! pass placed on the shared device where the guest's walker would draw
//! its quads, and the device's counts of the slots the guest ended
//! (docs/graphics/d3d-visibility.md, "The query in Rust").

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::flare_gpu::{DepthTest, FlareQueryGpu, QueryReadback};
use render_bridge::native_flare_query::{self, Counts, DeviceResult, Host, Quad};

use crate::native_lights_glue::DeviceMemory;

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
    quads: Vec<Quad>,
    test: DepthTest,
    counts: Arc<PassCounts>,
}

impl d3d::ExternalPass for FlareQueryPass {
    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: false, depth_stencil: false }
    }

    /// The W clip a W-buffered draw issued now takes: the walker's quads
    /// are drawn with `ZEnable(2)`.
    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.test = DepthTest { w_buffered: clip.w_buffered.unwrap_or(true), range: clip.range, clamp: clip.clamp };
    }

    /// Counted once, on the first copy recorded.
    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let mut slot = self.counts.0.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*slot, Recording::Waiting) {
            return;
        }
        let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
        if gpu.as_ref().is_none_or(|(d, _)| d != t.device) {
            *gpu = Some((t.device.clone(), FlareQueryGpu::new(t.device)));
        }
        let Some((_, pass)) = gpu.as_ref() else { return };
        *slot = match pass.record(t.device, t.encoder, t.depth.texture(), t.guest_size, self.test, t.z_scale, &self.quads) {
            Ok(r) => Recording::Recorded(r),
            Err(why) => {
                native_flare_query::QUERY.dropped(why_cause(why));
                Recording::Refused
            }
        };
    }
}

fn why_cause(why: &str) -> &'static str {
    if why.contains("depth") { "depth" } else { "empty" }
}

struct Query<B: d3d::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

impl<B: d3d::Backend + Send + 'static> Host for Query<B> {
    fn query(&self, mem: &mut dyn GuestMemory, quads: Vec<Quad>) -> Option<Arc<dyn Counts>> {
        let counts = Arc::new(PassCounts::default());
        let pass = FlareQueryPass {
            quads,
            test: DepthTest { w_buffered: true, range: [0.0, f32::MAX], clamp: true },
            counts: Arc::clone(&counts),
        };
        let mut device = self.device.lock().ok()?;
        matches!(device.external_pass_now(&mut DeviceMemory(mem), Box::new(pass)), Ok(true)).then(|| counts as Arc<dyn Counts>)
    }

    fn device_result(&self, mem: &mut dyn GuestMemory, slot: u32) -> DeviceResult {
        let Ok(mut device) = self.device.lock() else { return DeviceResult::Refused };
        match device.get_visibility_test_result(&mut DeviceMemory(mem), slot) {
            Ok(Some(a)) => DeviceResult::Ready(a.count),
            Ok(None) => DeviceResult::Pending,
            Err(_) => DeviceResult::Refused,
        }
    }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| native_flare_query::on(&s)) {
        return;
    }
    native_flare_query::set_host(Arc::new(Query { device: Arc::clone(device) }));
}
