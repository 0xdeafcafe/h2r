//! The native shadows' host: the shadow layers (10, 11, 14) bound to their
//! light or object call, and each object shadow's filter quads pushed as a
//! stage.
//!
//! The layers' draws were bound from the guest's resolved draws on its
//! bound targets; with no device those entries are named gaps.

use crate::native_shadow_chain::Chain;
use guest_layout::memory::GuestMemory;
use frame_types::{Slot, SlotAt};
use gfx::external::{ExternalAttachments, ExternalEncoded};
use native_render::{
    shadow_gpu::MapKind,
    shadows::Site,
    program::ViewInputs,
    stage::{FrameOp, Stage, StageWork, frame_builder},
};
use render_bridge::native_shadows::{self, Entry, Host, Layer};
use std::sync::{Arc, Mutex};

/// The frozen map of the receiver boundary that just ran, if it belongs
/// to this view, projection and map kind.
pub(crate) fn latest_receiver(
    _view: u32,
    _projection: [u32; engine::shadow_constants::BLOCK_WORDS],
    _kind: MapKind,
) -> Option<native_render::shadow_gpu::SampledMap> {
    unimplemented!("shadows: receiver map from a caster layer")
}

struct Glue;

/// Whether the native shadows are on (`native_shadows::on`).
pub(crate) fn enabled() -> bool {
    hooks::Switches::from_env().is_ok_and(|s| native_shadows::on(&s))
}

pub fn install() {
    if enabled() {
        native_shadows::set_host(Arc::new(Glue));
    } else {
        native_shadows::clear_host();
    }
}

/// 0x00014bc0 calls 0x0001dcf0 for the selected level. Its paired
/// surface for target 9 is the colour alias, and target 10 has no depth.
pub(crate) fn expected_targets(mem: &dyn GuestMemory, target: u32) -> Option<[u32; 2]> {
    use engine::shadow_constants::global::{TARGET_STRIDE, TARGETS};
    let base = TARGETS.checked_add(TARGET_STRIDE.checked_mul(target)?)?;
    if mem.u32(base + 0x88).ok()? == 0
        || mem.u8(base + 0x95).ok()? != 0
        || mem.i32(base + 0x78).ok()? <= 0
    {
        return None;
    }
    let surface = base + 0x18;
    match target {
        1 => Some([surface, mem.u32(0x0050_9364).ok()?]),
        9 => Some([mem.u32(0x0050_9384).ok()?, surface]),
        10 | 13 => Some([surface, 0]),
        _ => None,
    }
}

/// The level-0 header of one filter target.
pub(crate) fn target_header(mem: &dyn GuestMemory, index: u32) -> Option<u32> {
    let descriptor = engine::shadow_constants::global::TARGETS
        .checked_add(index.checked_mul(engine::shadow_constants::global::TARGET_STRIDE)?)?;
    if mem.u32(descriptor + 0x88).ok()? == 0
        || mem.u8(descriptor + 0x95).ok()? != 0
        || mem.i32(descriptor + 0x78).ok()? <= 0
    {
        return None;
    }
    Some(descriptor + 0x18)
}

/// Where a filter pass's quad programs live.
pub(crate) enum FilterDrawer {
    /// One drawer per device, for an object shadow drawn from the scene
    /// (`native_lights_glue::scene`).
    Scene,
}

impl FilterDrawer {
    fn with<R>(
        &self,
        device: &wgpu::Device,
        f: impl FnOnce(&mut native_render::glow::QuadDrawer) -> R,
    ) -> Result<R, &'static str> {
        type Drawers = Vec<(wgpu::Device, Arc<Mutex<native_render::glow::QuadDrawer>>)>;
        static SCENE: Mutex<Drawers> = Mutex::new(Vec::new());
        match self {
            Self::Scene => {
                let drawer = {
                    let mut all = SCENE.lock().unwrap_or_else(|e| e.into_inner());
                    match all.iter().find(|(d, _)| d == device) {
                        Some((_, drawer)) => Arc::clone(drawer),
                        None => {
                            let drawer = Arc::new(Mutex::new(native_render::glow::QuadDrawer::new()));
                            all.push((device.clone(), Arc::clone(&drawer)));
                            drawer
                        }
                    }
                };
                let mut drawer = drawer.lock().map_err(|_| "shadow filter drawer poisoned")?;
                Ok(f(&mut drawer))
            }
        }
    }
}

/// An object shadow's filter quads (`0x000222a0`) between targets 10 and
/// 11, or a private chain's pair.
pub(crate) struct ShadowFilterPass {
    pub filter: frame_scene::ShadowFilter,
    pub drawer: FilterDrawer,
    /// A private chain's pair stands in for targets 10 and 11.
    pub chain: Option<Arc<Chain>>,
}

impl ShadowFilterPass {
    fn prepare(&self, t: &ExternalAttachments) -> Result<ExternalEncoded, &'static str> {
        let Some(chain) = &self.chain else {
            unimplemented!("shadows: filter targets 10 and 11 from the frame graph")
        };
        let pair = chain.pair_for(t.size).ok_or("private shadow pair missing")?;
        let encoded = self.drawer.with(&t.device, |drawer| {
            native_render::shadow_filter_gpu::prepare(
                &t.device,
                &t.queue,
                &t.uploads,
                drawer,
                &self.filter,
                pair.filter_slots(),
            )
        })??;
        chain.publish(&pair);
        Ok(encoded)
    }
}

impl Stage for ShadowFilterPass {
    fn name(&self) -> &'static str {
        "shadow_filter"
    }

    fn record(&mut self, t: &ExternalAttachments, _view: &ViewInputs) -> StageWork {
        match self.prepare(t) {
            Ok(encoded) => StageWork::Encoded(encoded),
            Err(why) => panic!("shadows: filter pass: {why}"),
        }
    }
}

impl Host for Glue {
    fn parent(&self, _site: Option<Site>, _identity: Option<[u32; 2]>) {
        unimplemented!("shadows: layer owner from the device's bound draws")
    }

    fn entry(&self, _mem: &mut dyn GuestMemory, _entry: Option<Entry>) {
        unimplemented!("shadows: geometry entry from the device's bound draws")
    }

    fn begin(&self, _mem: &mut dyn GuestMemory, _layer: Layer) {
        unimplemented!("shadows: layer targets from the device")
    }

    fn end(&self, _mem: &mut dyn GuestMemory, _site: Site, _layer: u32) {
        unimplemented!("shadows: layer targets from the device")
    }

    fn filter(
        &self,
        _mem: &mut dyn GuestMemory,
        _esp: u32,
        filter: frame_scene::ShadowFilter,
    ) -> Result<(), &'static str> {
        let pass = ShadowFilterPass { filter, drawer: FilterDrawer::Scene, chain: None };
        frame_builder().push(FrameOp::Stage {
            slot: SlotAt { slot: Slot::AfterLights, view: 0 },
            stage: Box::new(pass),
        });
        Ok(())
    }

    fn draw_alone(
        &self,
        mem: &mut dyn GuestMemory,
        view: u32,
        group: &render_bridge::native_lights::SceneGroup,
    ) -> Result<(), &'static str> {
        render_bridge::native_lights::draw_shadow_alone(mem, view, group)
    }
}
