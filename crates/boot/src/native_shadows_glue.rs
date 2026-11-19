//! Compare layer 10 on private GPU attachments while the original keeps
//! producing the guest-visible map. Begin/end markers travel through the
//! same command queue as the recorded batches, including the scaled copy.

use crate::{
    native_lights_glue::DeviceMemory,
    native_shadows_draw::{self, Freeze, SharedGpu},
    native_shadows_scope::{Call as Scope, Gate, State},
};
use engine::memory::GuestMemory;
use feature::native_shadows::{self, Entry, Host, Layer};
use gfx::{Clear, GfxContext, RecordedDraw, RenderTarget, depth_bits::DepthBits};
use native_render::{
    shadow_gpu::{MapKind, Producer},
    shadows::{self, Site},
};
use std::cell::RefCell;
use std::sync::{
    Arc, Mutex, Weak,
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
    feature::Switches::from_env().is_ok_and(|s| native_shadows::on(&s))
}

static SINGLE_GPU: Mutex<Option<SharedGpu>> = Mutex::new(None);
pub(crate) fn configure_single(backend: &mut d3d::GfxBackend) {
    if enabled() && !native_shadows::comparing() {
        let gpu = native_shadows_draw::for_backend(backend);
        *SINGLE_GPU.lock().unwrap_or_else(|e| e.into_inner()) = Some(gpu);
    }
}

pub(crate) fn install<B: d3d::backend::Backend + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let gpu = SINGLE_GPU.lock().unwrap_or_else(|e| e.into_inner()).take();
    let compare = native_shadows::comparing();
    if enabled() && (compare || gpu.is_some()) {
        let gate = Arc::new(Gate {
            state: Mutex::new(State::default()),
            gpu,
            compare,
        });
        device
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .add_draw_admission(gate.clone());
        native_shadows::set_host(Arc::new(Glue {
            device: Arc::clone(device),
            gate,
            sequence: AtomicU64::new(1),
        }));
    } else {
        native_shadows::clear_host();
    }
}

pub(crate) fn comparison_enabled() -> bool {
    enabled() && native_shadows::comparing()
}

pub(crate) fn configure(backend: &mut d3d::GfxBackend, copy: &'static str) {
    if comparison_enabled() {
        let mut bits = None;
        backend.observe_recorded_pass(move |ctx, enc, source, target, clear, draws| {
            batch(ctx, enc, source, target, clear, draws, copy, &mut bits);
        });
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
        if self.gate.compare
            && let (Some(s), Some(e)) = (&state.active, state.entry)
        {
            eprintln!(
                "native-shadows frame={} seq={} sorted item={} shader={:#x} arg={:#x} impl={}",
                s.frame, s.seq, e.index, e.shader, e.argument, e.implementation
            );
        }
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
        if self.gate.compare {
            eprintln!(
                "native-shadows frame={frame} seq={seq} {:?} layer={} view={} target={} bound={bound:x?} formats={colour:?}/{depth:?} flags={:#x} candidates={} projection={:08x?}",
                layer.site,
                layer.layer,
                layer.view,
                layer.target,
                layer.flags,
                layer.items.len(),
                layer.projection
            );
            for item in &layer.items {
                eprintln!("native-shadows frame={frame} seq={seq} candidate={item:?}");
            }
            eprintln!(
                "native-shadows frame={frame} seq={seq} inherited stencil={:?} shadowfunc={:?} bias={:x?}",
                d.stencil_state(&mem),
                d.render_state(&mem, d3d::state::slot::SHADOWFUNC),
                [
                    d3d::state::slot::SOLID_OFFSET_ENABLE,
                    d3d::state::slot::POLYGON_OFFSET_ZOFFSET,
                    d3d::state::slot::POLYGON_OFFSET_ZSLOPESCALE
                ]
                .map(|i| d.render_state(&mem, i))
            );
        }
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
            if !self.gate.compare
                && layer
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
                recorded: std::env::var("H2_NATIVE_SHADOWS_PROBE")
                    .is_ok_and(|v| v == "1")
                    .then(Default::default),
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
            return;
        }
        if is_caster && self.gate.compare {
            d.request_external_pass(Box::new(Marker {
                scope: call,
                begin: true,
            }));
            let _ = d.begin_external_boundary(&mut mem);
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
            if let Some(count) = &c.recorded {
                eprintln!(
                    "native-shadows frame={} seq={} view={} site={:?} owner={:x?} layer={} recorded_ranges={}",
                    c.frame,
                    c.seq,
                    c.layer.view,
                    c.layer.site,
                    c.identity,
                    layer,
                    count.load(Ordering::Relaxed)
                );
            }
            if layer != 10 {
                s.previous = None;
                return;
            }
            s.previous = Some(Arc::clone(&c));
            c
        };
        if self.gate.compare {
            d.request_external_pass(Box::new(Marker {
                scope: call,
                begin: false,
            }));
        } else if call.kind == MapKind::Depth
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

struct Marker {
    scope: Arc<Scope>,
    begin: bool,
}
struct Copy {
    scope: Weak<Scope>,
    colour: wgpu::Texture,
    depth: Option<wgpu::Texture>,
    size: (u32, u32),
    producer: Option<Producer>,
    copy: Option<&'static str>,
    context: Option<GfxContext>,
    bits: Option<Arc<DepthBits>>,
    refused: bool,
}

thread_local! {
    // Distinct physical images for the reference and scaled view, even at
    // 1x. A dropped end marker releases its token and prevents further work.
    static COPIES: RefCell<Vec<Copy>> = const { RefCell::new(Vec::new()) };
}

impl d3d::ExternalPass for Marker {
    fn colour_only(&self) -> bool {
        self.scope.bound[1] == 0
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        COPIES.with_borrow_mut(|copies| {
            copies.retain(|c| c.scope.strong_count() > 0);
            let colour = t.color.texture();
            let depth = (self.scope.bound[1] != 0).then(|| t.depth.texture());
            if self.begin {
                copies.retain(|c| &c.colour != colour);
                copies.push(Copy {
                    scope: Arc::downgrade(&self.scope),
                    colour: colour.clone(),
                    depth: depth.cloned(),
                    size: t.size,
                    producer: None,
                    copy: None,
                    context: None,
                    bits: None,
                    refused: false,
                });
            } else if let Some(at) = copies.iter().position(|c| {
                c.scope.ptr_eq(&Arc::downgrade(&self.scope))
                    && &c.colour == colour
                    && c.depth.as_ref() == depth
            }) {
                let c = copies.swap_remove(at);
                if !c.finish(t.encoder, &self.scope) {
                    eprintln!(
                        "native-shadows frame={} seq={} no comparable caster batch",
                        self.scope.frame, self.scope.seq
                    );
                }
            }
        });
    }
}

impl Copy {
    fn matches(&self, source: &wgpu::Texture, t: &RenderTarget<'_>) -> bool {
        self.size == t.size
            && self.depth.as_ref() == t.depth.map(|v| v.texture())
            && source == &self.colour
    }

    fn finish(self, enc: &mut wgpu::CommandEncoder, s: &Scope) -> bool {
        if self.refused {
            return false;
        }
        let (Some(producer), Some(ctx), Some(bits)) = (self.producer, self.context, self.bits)
        else {
            return false;
        };
        let draws = producer.draws();
        let Some(map) = producer.finish(&ctx, enc, &bits) else {
            return false;
        };
        let reference = match s.kind {
            MapKind::ColourMask => self.colour,
            MapKind::Depth => {
                let Some(t) = self.depth else { return false };
                if !t.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING) {
                    return false;
                }
                bits.convert(&ctx, enc, &t, (t.width(), t.height()))
            }
        };
        let Some(copy) = self.copy else { return false };
        let report = crate::native_shadows_probe::MapComparison {
            frame: s.frame,
            generation: s.seq,
            view: s.layer.view,
            identity: s.identity,
            copy,
            site: match s.layer.site {
                Site::Light => "light",
                Site::AdditionalCaster => "additional",
                Site::Object => "object",
            },
            kind: match s.kind {
                MapKind::ColourMask => "colour_mask",
                MapKind::Depth => "depth",
            },
            candidates: s.layer.items.len(),
            projection: std::array::from_fn(|r| {
                std::array::from_fn(|i| s.layer.projection[4 * r + i])
            }),
            size: [map.size.0, map.size.1],
            packets: draws,
            texels_differ: 0,
            first: None,
        };
        compare(&ctx.device, enc, map.view.texture(), &reference, report)
    }
}

fn batch(
    ctx: &GfxContext,
    enc: &mut wgpu::CommandEncoder,
    source: &wgpu::Texture,
    target: &RenderTarget<'_>,
    clear: Clear,
    draws: &[RecordedDraw],
    copy: &'static str,
    bits: &mut Option<Arc<DepthBits>>,
) {
    COPIES.with_borrow_mut(|copies| {
        copies.retain(|c| c.scope.strong_count() > 0);
        let Some(c) = copies.iter_mut().find(|c| c.matches(source, target)) else {
            return;
        };
        if c.refused {
            return;
        }
        let Some(scope) = c.scope.upgrade() else {
            return;
        };
        if c.producer.is_none() {
            c.copy = Some(copy);
            c.producer = Producer::new(&ctx.device, enc, target, scope.kind);
            c.context = Some(GfxContext {
                instance: ctx.instance.clone(),
                adapter: ctx.adapter.clone(),
                device: ctx.device.clone(),
                queue: ctx.queue.clone(),
                caps: ctx.caps,
            });
            c.bits = Some(Arc::clone(
                bits.get_or_insert_with(|| Arc::new(DepthBits::new(ctx))),
            ));
        }
        let Some(p) = c.producer.as_mut() else {
            c.refused = true;
            return;
        };
        for (n, d) in draws.iter().enumerate() {
            eprintln!(
                "native-shadows frame={} seq={} packet={} {}",
                scope.frame,
                scope.seq,
                p.draws() + n,
                d.describe()
            );
        }
        if let Err(why) = p.record(ctx, enc, clear, draws) {
            c.refused = true;
            eprintln!(
                "native-shadows frame={} seq={} refused: {why}",
                scope.frame, scope.seq
            );
        }
    });
}

fn compare(
    device: &wgpu::Device,
    enc: &mut wgpu::CommandEncoder,
    native: &wgpu::Texture,
    reference: &wgpu::Texture,
    mut report: crate::native_shadows_probe::MapComparison,
) -> bool {
    let (w, h) = (native.width(), native.height());
    if native.size() != reference.size()
        || native.format() != reference.format()
        || !reference.usage().contains(wgpu::TextureUsages::COPY_SRC)
    {
        return false;
    }
    let pitch = (4 * w).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let half = u64::from(pitch) * u64::from(h);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shadow-map compare"),
        size: 2 * half,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for (texture, offset) in [(native, 0), (reference, half)] {
        enc.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset,
                    bytes_per_row: Some(pitch),
                    rows_per_image: Some(h),
                },
            },
            native.size(),
        );
    }
    let result = buf.clone();
    enc.map_buffer_on_submit(&buf, wgpu::MapMode::Read, .., move |r| {
        if r.is_err() {
            eprintln!("native-shadows frame={} seq={} readback failed", report.frame, report.generation);
            return;
        }
        let Ok(data) = result.slice(..).get_mapped_range() else {
            return;
        };
        let (a, b) = data.split_at(half as usize);
        let mut differences = 0;
        let mut first = None;
        for (y, (a, b)) in a
            .chunks_exact(pitch as usize)
            .zip(b.chunks_exact(pitch as usize))
            .enumerate()
        {
            for (x, (a, b)) in a
                .chunks_exact(4)
                .zip(b.chunks_exact(4))
                .take(w as usize)
                .enumerate()
            {
                if a != b {
                    differences += 1;
                    first.get_or_insert([x, y]);
                }
            }
        }
        report.size = [w,h];
        report.texels_differ = differences;
        report.first = first;
        eprintln!("native-shadows frame={} seq={} {} {} copy={} packets={} map={w}x{h} texels_differ={differences} first={first:?}",
            report.frame, report.generation, report.site, report.kind, report.copy, report.packets);
        crate::native_shadows_probe::record(report);
    });
    true
}
