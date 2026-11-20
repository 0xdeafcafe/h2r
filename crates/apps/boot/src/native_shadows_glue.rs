//! The native shadows' host: binds each shadow layer (10, 11, 14) to its
//! light or object call, admits the layer's draws to the native shadow
//! passes (`native_shadows_scope::Gate`), freezes a caster's map for its
//! receivers and hands each receiver term to the lights.

use crate::{
    native_lights_glue::DeviceMemory,
    native_shadows_draw::{self, Freeze, SharedGpu},
    native_shadows_scope::{Call as Scope, Gate, State},
};
use engine::memory::GuestMemory;
use hooks::native_shadows::{self, Entry, Host, Layer};
use native_render::{
    shadow_gpu::MapKind,
    shadows::{self, Site},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

/// A receiver's sampled term. The frame, view, parent identity and
/// sequence identify one generation even when a light calls several pairs.
#[derive(Clone)]
pub struct ReceiverTerm {
    pub frame: u64,
    pub view: u32,
    pub generation: u64,
    pub site: Site,
    /// Light datum and definition datum, or the two object-shadow arguments.
    pub identity: [u32; 2],
    pub projection: [u32; engine::shadow_constants::BLOCK_WORDS],
    pub kind: MapKind,
    pub texture_header: u32,
    output: native_shadows_draw::Output,
}
impl ReceiverTerm {
    /// Resolve when recording after the caster/filter in GPU order.
    /// An unavailable term keeps the light's original chain.
    pub fn sampled(&self) -> Option<native_render::shadow_gpu::SampledMap> {
        self.output.lock().ok()?.clone()
    }
}
type ReceiverObserver = Arc<dyn Fn(ReceiverTerm) + Send + Sync>;
static RECEIVER: Mutex<Option<ReceiverObserver>> = Mutex::new(None);
/// Observe generation handles on the guest render thread before layer 11.
pub fn observe_receivers(observer: Option<Arc<dyn Fn(ReceiverTerm) + Send + Sync>>) {
    *RECEIVER.lock().unwrap_or_else(|e| e.into_inner()) = observer;
}

struct Glue<B: d3d::backend::Backend> {
    device: d3d::abi::SharedDevice<B>,
    gate: Arc<Gate>,
    sequence: AtomicU64,
}
fn enabled() -> bool {
    hooks::Switches::from_env().is_ok_and(|s| native_shadows::on(&s))
}

static SINGLE_GPU: Mutex<Option<SharedGpu>> = Mutex::new(None);
pub(crate) fn configure_single(backend: &mut d3d::GfxBackend) {
    if enabled() {
        let gpu = native_shadows_draw::for_backend(backend);
        *SINGLE_GPU.lock().unwrap_or_else(|e| e.into_inner()) = Some(gpu);
    }
}

pub(crate) fn install<B: d3d::backend::Backend + 'static>(device: &d3d::abi::SharedDevice<B>) -> Option<Arc<dyn d3d::DrawAdmission>> {
    let gpu = SINGLE_GPU.lock().unwrap_or_else(|e| e.into_inner()).take();
    if enabled() && gpu.is_some() {
        let gate = Arc::new(Gate {
            state: Mutex::new(State::default()),
            gpu,
        });
        device
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .add_draw_admission(gate.clone());
        native_shadows::set_host(Arc::new(Glue {
            device: Arc::clone(device),
            gate: Arc::clone(&gate),
            sequence: AtomicU64::new(1),
        }));
        Some(gate)
    } else {
        native_shadows::clear_host();
        None
    }
}

fn format(mem: &DeviceMemory<'_>, header: u32) -> Option<gfx::D3DFormat> {
    if header == 0 {
        return None;
    }
    let h = d3d::resource::PixelContainer::read(mem, header).ok()?;
    Some(gfx::TextureDesc::from_header(h.format, h.size).ok()?.format)
}

/// 0x00014bc0 calls 0x0001dcf0 for the selected level. Its paired
/// surface for target 9 is the colour alias, and target 10 has no depth.
fn expected_targets(mem: &dyn GuestMemory, target: u32) -> Option<[u32; 2]> {
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
        10 => Some([surface, 0]),
        _ => None,
    }
}

impl<B: d3d::backend::Backend + 'static> Host for Glue<B> {
    fn parent(&self, site: Option<Site>, identity: Option<[u32; 2]>) {
        let mut state = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        let identity = if site == Some(Site::AdditionalCaster) && state.owner == Some(Site::Light) {
            state.identity
        } else {
            identity
        };
        *state = State {
            owner: site,
            identity,
            ..Default::default()
        };
    }

    fn entry(&self, mem: &mut dyn GuestMemory, entry: Option<Entry>) {
        // Flush old geometry with its prior identity, before advancing the cursor.
        if self
            .gate
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active
            .is_none()
        {
            return;
        }
        let mut d = self.device.lock().unwrap_or_else(|e| e.into_inner());
        let drained = matches!(d.begin_external_boundary(&mut DeviceMemory(mem)), Ok(true));
        let mut state = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        state.entry = if drained { entry } else { None };
    }

    fn begin(&self, mem: &mut dyn GuestMemory, layer: Layer) {
        let mut d = self.device.lock().unwrap_or_else(|e| e.into_inner());
        let drained = matches!(d.begin_external_boundary(&mut DeviceMemory(mem)), Ok(true));
        let frame = crate::d3d_diag::presents() + 1;
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let bound = d.bound_target();
        let expected = matches!(layer.layer, 10 | 14)
            .then(|| expected_targets(mem, layer.target))
            .flatten();
        let mut mem = DeviceMemory(mem);
        let colour = format(&mem, bound[0]);
        let depth = format(&mem, bound[1]);
        let is_caster = layer.layer == 10;
        let kind = if shadows::colour_mask(layer.flags) {
            MapKind::ColourMask
        } else {
            MapKind::Depth
        };
        let target = match kind {
            MapKind::ColourMask => 10,
            MapKind::Depth => 9,
        };
        let header = engine::shadow_constants::global::TARGETS
            + target * engine::shadow_constants::global::TARGET_STRIDE
            + 4;
        let (call, previous) = {
            let mut s = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
            s.active = None;
            s.entry = None;
            if is_caster || layer.layer == 14 {
                s.previous = None;
            }
            if layer.layer == 14 && s.owner == Some(Site::AdditionalCaster) {
                s.owner = Some(Site::Light);
            }
            if !drained
                || s.owner != Some(layer.site)
                || (layer.layer == 14
                    && (layer.target != 1 || expected != Some([bound[0], bound[1]])))
                || !d.can_external_pass()
            {
                return;
            }
            if is_caster {
                if expected != Some([bound[0], bound[1]])
                    || match kind {
                        MapKind::ColourMask => !matches!(
                            colour,
                            Some(gfx::D3DFormat::LinA8R8G8B8 | gfx::D3DFormat::A8R8G8B8)
                        ),
                        MapKind::Depth => !matches!(
                            depth,
                            Some(gfx::D3DFormat::LinD24S8 | gfx::D3DFormat::D24S8)
                        ),
                    }
                {
                    return;
                }
            }
            if layer
                .items
                .iter()
                .any(|i| matches!(i.geometry, shadows::Geometry::Other { .. }))
            {
                return;
            }
            let Some(identity) = s.identity else { return };
            let c = Arc::new(Scope {
                identity,
                seq,
                frame,
                bound,
                layer,
                kind,
                header,
                output: Default::default(),
            });
            s.active = Some(Arc::clone(&c));
            (c, s.previous.clone())
        };
        if call.layer.layer == 11
            && let Some(source) = previous
            && source.supplies(&call)
            && let Some(gpu) = &self.gate.gpu
        {
            if source.kind == MapKind::ColourMask {
                // Freeze after the object's filter at 0x222a0.
                d.request_external_pass(Box::new(Freeze {
                    gpu: gpu.clone(),
                    output: source.output.clone(),
                    kind: source.kind,
                    sampled: Some(source.header),
                }));
                if !matches!(d.begin_external_boundary(&mut mem), Ok(true)) {
                    return;
                }
            }
            drop(d);
            let observer = RECEIVER.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(observe) = observer {
                observe(ReceiverTerm {
                    frame,
                    view: call.layer.view,
                    generation: source.seq,
                    site: source.layer.site,
                    identity: source.identity,
                    projection: source.layer.projection,
                    kind: source.kind,
                    texture_header: source.header,
                    output: source.output.clone(),
                });
            }
        }
    }
    fn end(&self, mem: &mut dyn GuestMemory, site: Site, layer: u32) {
        let mut d = self.device.lock().unwrap_or_else(|e| e.into_inner());
        let mut mem = DeviceMemory(mem);
        let drained = matches!(d.begin_external_boundary(&mut mem), Ok(true));
        let call = {
            let mut s = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
            s.entry = None;
            let Some(c) = s.active.take() else { return };
            if !drained
                || c.layer.layer != layer
                || c.layer.site != site
                || c.bound != d.bound_target()
            {
                s.previous = None;
                return;
            }
            if layer != 10 {
                s.previous = None;
                return;
            }
            s.previous = Some(Arc::clone(&c));
            c
        };
        if call.kind == MapKind::Depth
            && let Some(gpu) = &self.gate.gpu
        {
            d.request_external_pass(Box::new(Freeze {
                gpu: gpu.clone(),
                output: call.output.clone(),
                kind: call.kind,
                sampled: None,
            }));
        }
        let _ = d.begin_external_boundary(&mut mem);
    }
}
