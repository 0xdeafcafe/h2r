//! The native shadows' host: binds each shadow layer (10, 11, 14) to its
//! light or object call, admits the layer's draws to the native shadow
//! passes (`native_shadows_scope::Gate`), freezes a caster's map for its
//! receivers and hands each receiver term to the lights.

use crate::{
    native_lights_glue::DeviceMemory,
    native_shadow_chain::{self, Chain, Spill},
    native_shadows_draw::{self, Freeze, SharedGpu},
    native_shadows_scope::{Call as Scope, Gate, State},
};
use engine::memory::GuestMemory;
use native_render::{
    shadow_gpu::MapKind,
    shadows::{self, Site},
};
use render_bridge::native_shadows::{self, Entry, Host, Layer};
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

static LATEST: Mutex<Option<ReceiverTerm>> = Mutex::new(None);

/// The frozen map of the receiver boundary that just ran, if it belongs
/// to this view, projection and map kind.
pub(crate) fn latest_receiver(
    view: u32,
    projection: [u32; engine::shadow_constants::BLOCK_WORDS],
    kind: MapKind,
) -> Option<native_render::shadow_gpu::SampledMap> {
    let term = LATEST.lock().ok()?.clone()?;
    if term.view != view || term.projection != projection || term.kind != kind {
        return None;
    }
    term.sampled()
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
pub fn configure_single(backend: &mut d3d::GfxBackend) {
    if enabled() {
        let gpu = native_shadows_draw::for_backend(backend);
        *SINGLE_GPU.lock().unwrap_or_else(|e| e.into_inner()) = Some(gpu);
    }
}

pub fn install<B: d3d::backend::Backend + 'static>(
    device: &d3d::abi::SharedDevice<B>,
) -> Option<Arc<dyn d3d::DrawAdmission>> {
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
        10 => Some([surface, 0]),
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

struct ShadowFilterPass {
    filter: frame_scene::ShadowFilter,
    gpu: SharedGpu,
    /// A private chain's pair stands in for targets 10 and 11.
    chain: Option<Arc<Chain>>,
}

impl ShadowFilterPass {
    fn prepare(
        &self,
        t: &d3d::backend::ExternalAttachments,
    ) -> Result<d3d::backend::ExternalEncoded, &'static str> {
        if let Some(chain) = &self.chain {
            let pair = chain.pair().ok_or("private shadow pair missing")?;
            let mut gpu = self.gpu.lock().map_err(|_| "shadow filter GPU poisoned")?;
            let encoded = native_render::shadow_filter_gpu::prepare(
                &t.device,
                &t.queue,
                &t.uploads,
                gpu.filter(),
                &self.filter,
                pair.filter_slots(),
            )?;
            chain.publish(&pair);
            return Ok(encoded);
        }
        let slots = [0, 1].map(|n| {
            t.slots
                .get(n)
                .and_then(Option::as_ref)
                .map(native_render::glow::PostSurface::from)
        });
        let [Some(mask), Some(other)] = slots else {
            return Err("shadow filter target is unavailable");
        };
        let mut gpu = self.gpu.lock().map_err(|_| "shadow filter GPU poisoned")?;
        native_render::shadow_filter_gpu::prepare(
            &t.device,
            &t.queue,
            &t.uploads,
            gpu.filter(),
            &self.filter,
            [mask, other],
        )
    }
}

impl d3d::ExternalPass for ShadowFilterPass {
    fn colour_only(&self) -> bool {
        true
    }

    fn uses_validated_resources(&self) -> bool {
        true
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: self.chain.is_none(),
            depth_stencil: false,
        }
    }

    fn record_encoded(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        _: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        let result = self.prepare(t);
        if let (Err(why), Some(_)) = (&result, &self.chain) {
            // The guest's quads are dropped and the receivers wait on this
            // chain's mask: the generation draws no shadow.
            native_shadows::dropped_layer();
            eprintln!("native-shadows: a private filter failed while recording: {why}");
        }
        Some(result)
    }
}

impl d3d::SwallowPass for ShadowFilterPass {
    fn validate(
        &self,
        originals: &[d3d::SwallowOriginal<'_>],
        slots: &[Option<d3d::SwallowTarget>],
    ) -> Result<(), String> {
        if originals.len() != self.filter.draws.len() {
            return Err("shadow filter draw count differs".into());
        }
        for (original, draw) in originals.iter().zip(&self.filter.draws) {
            if !crate::native_post_glue::matches_colour_state(
                original.draw,
                None,
                wgpu::ColorWrites::ALL,
                None,
            ) {
                return Err("shadow filter colour state differs".into());
            }
            let slot = match draw.source {
                10 => 0,
                11 => 1,
                _ => return Err("shadow filter source is not target 10 or 11".into()),
            };
            for stage in 0..4 {
                if original.sampled[stage] {
                    crate::native_post_glue::check_bound_source(
                        original,
                        stage,
                        Some(slot),
                        slots,
                        false,
                    )?;
                }
            }
        }
        Ok(())
    }
}

impl<B: d3d::backend::Backend + 'static> Glue<B> {
    /// The running layer-10 call, when its whole group is an object mask
    /// that may draw into a private chain.
    fn private_candidate(
        &self,
        group: &render_bridge::native_lights::SceneGroup,
    ) -> Option<(Arc<Scope>, Arc<Chain>)> {
        let pass = group.group.pass?;
        if group.group.site != frame_scene::GroupSite::ObjectShadow
            || group.group.layer != 10
            || !pass.colour_mask()
        {
            return None;
        }
        let gpu = self.gate.gpu.as_ref()?;
        let call = self.gate.state.lock().ok()?.active.clone()?;
        if call.layer.layer != 10
            || call.layer.site != Site::Object
            || call.kind != MapKind::ColourMask
        {
            return None;
        }
        let ctx = gpu.lock().ok()?.context();
        let chain = Chain::new(ctx, call.output.clone());
        Some((call, chain))
    }

    /// The last caster call and its private chain, while it holds one.
    fn private_previous(&self) -> Option<(Arc<Scope>, Arc<Chain>)> {
        let call = self.gate.state.lock().ok()?.previous.clone()?;
        let chain = call.private_chain()?;
        Some((call, chain))
    }

    /// Hand a private chain's casters back to target 10, before any guest
    /// draw reads it. From here the chain is the guest's.
    fn spill(&self, mem: &mut dyn GuestMemory, call: &Scope, chain: Arc<Chain>) {
        if !chain.spill_once() {
            return;
        }
        native_shadows::chain_spilled();
        let mut d = self.device.lock().unwrap_or_else(|e| e.into_inner());
        d.request_external_pass(Box::new(Spill {
            chain,
            header: call.header,
        }));
        let _ = d.begin_external_boundary(&mut DeviceMemory(mem));
    }

    fn swallow_filter(
        &self,
        mem: &mut dyn GuestMemory,
        esp: u32,
        filter: frame_scene::ShadowFilter,
        chain: Option<Arc<Chain>>,
    ) -> Result<(), &'static str> {
        let gpu = self.gate.gpu.clone().ok_or("device")?;
        let headers = vec![target_header(mem, 10), target_header(mem, 11)];
        if headers.iter().any(Option::is_none) {
            return Err("target");
        }
        let targets = filter
            .draws
            .iter()
            .map(|draw| match draw.dest {
                10 => 0,
                11 => 1,
                _ => u8::MAX,
            })
            .collect::<Vec<_>>();
        if targets.contains(&u8::MAX) {
            return Err("target");
        }
        let mut device = self.device.lock().map_err(|_| "device")?;
        device
            .swallow(
                &mut DeviceMemory(mem),
                d3d::Swallow {
                    call: Some(d3d::SwallowCall {
                        entry: engine::points::OBJECT_SHADOW_FILTER.base,
                        esp,
                        ret: engine::render::light_gather::object_shadows::caster::FILTER_RETURN,
                    }),
                    end_returns: Vec::new(),
                    headers,
                    targets,
                    primitive: gfx::vertex::PrimitiveType::TriangleFan,
                    vertices: 4,
                    shapes: Vec::new(),
                    drop: true,
                    pass: Box::new(ShadowFilterPass { filter, gpu, chain }),
                },
            )
            .map_err(|_| "device")?
            .then_some(())
            .ok_or("device")
    }
}

impl<B: d3d::backend::Backend + 'static> Host for Glue<B> {
    fn parent(&self, site: Option<Site>, identity: Option<[u32; 2]>) {
        *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
        *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
                private: Default::default(),
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
                // Freeze after the object's filter at 0x222a0. A private
                // chain's filter publishes its own mask instead.
                if source.private_chain().is_none() {
                    d.request_external_pass(Box::new(Freeze {
                        gpu: gpu.clone(),
                        output: source.output.clone(),
                        kind: source.kind,
                        sampled: Some(source.header),
                    }));
                }
                if !matches!(d.begin_external_boundary(&mut mem), Ok(true)) {
                    return;
                }
            }
            drop(d);
            let term = ReceiverTerm {
                frame,
                view: call.layer.view,
                generation: source.seq,
                site: source.layer.site,
                identity: source.identity,
                projection: source.layer.projection,
                kind: source.kind,
                texture_header: source.header,
                output: source.output.clone(),
            };
            *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = Some(term.clone());
            let observer = RECEIVER.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(observe) = observer {
                observe(term);
            }
        }
    }

    fn filter(
        &self,
        mem: &mut dyn GuestMemory,
        esp: u32,
        filter: frame_scene::ShadowFilter,
    ) -> Result<(), &'static str> {
        let private = self.private_previous();
        let chain = private.as_ref().map(|(_, chain)| Arc::clone(chain));
        let result = self.swallow_filter(mem, esp, filter, chain);
        if result.is_err()
            && let Some((call, chain)) = private
        {
            // The guest's quads run: they must read the casters.
            self.spill(mem, &call, chain);
        }
        result
    }

    fn filter_kept(&self, mem: &mut dyn GuestMemory) {
        if let Some((call, chain)) = self.private_previous() {
            self.spill(mem, &call, chain);
        }
    }

    fn finish_filter(&self, mem: &mut dyn GuestMemory, esp: u32) -> Result<(), &'static str> {
        let mut device = self.device.lock().map_err(|_| "device")?;
        device
            .finish_swallow(
                &mut DeviceMemory(mem),
                engine::points::OBJECT_SHADOW_FILTER.base,
                esp,
            )
            .map_err(|_| "device")
    }

    /// An object mask's whole caster group goes into a private chain when
    /// the light host claims the offer with the pass it commits.
    fn draw_alone(
        &self,
        mem: &mut dyn GuestMemory,
        view: u32,
        group: &render_bridge::native_lights::SceneGroup,
    ) -> Result<(), &'static str> {
        let candidate = self.private_candidate(group);
        if let Some((_, chain)) = &candidate {
            native_shadow_chain::offer(Arc::clone(chain));
        }
        let result = render_bridge::native_lights::draw_shadow_alone(mem, view, group);
        let unclaimed = native_shadow_chain::withdraw();
        if result.is_ok()
            && unclaimed.is_none()
            && let Some((call, chain)) = candidate
        {
            *call.private.lock().unwrap_or_else(|e| e.into_inner()) = Some(chain);
            native_shadows::chain_private();
        }
        result
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
                // The filter will not know this chain: it reads target 10.
                // Drain without the state lock; the gate takes it.
                let spill = c.private_chain().filter(|chain| chain.spill_once());
                drop(s);
                if let Some(chain) = spill {
                    native_shadows::chain_spilled();
                    d.request_external_pass(Box::new(Spill {
                        chain,
                        header: c.header,
                    }));
                    let _ = d.begin_external_boundary(&mut mem);
                }
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
