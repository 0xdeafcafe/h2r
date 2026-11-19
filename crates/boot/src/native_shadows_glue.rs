//! Compare layer 10 on private GPU attachments while the original keeps
//! producing the guest-visible map. Begin/end markers travel through the
//! same command queue as the recorded batches, including the scaled copy.

use crate::native_lights_glue::DeviceMemory;
use engine::memory::GuestMemory;
use feature::native_shadows::{self, Host, Layer};
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

struct Scope {
    seq: u64,
    site: Site,
    kind: MapKind,
    frame: u64,
    bound: [u32; 8],
}

struct Glue<B: d3d::backend::Backend> {
    device: d3d::abi::SharedDevice<B>,
    active: Mutex<Option<Arc<Scope>>>,
    sequence: AtomicU64,
}

fn enabled() -> bool {
    feature::Switches::from_env().is_ok_and(|s| native_shadows::on(&s))
        && native_shadows::comparing()
}

pub(crate) fn install<B: d3d::backend::Backend + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if enabled() {
        native_shadows::set_host(Arc::new(Glue {
            device: Arc::clone(device),
            active: Mutex::new(None),
            sequence: AtomicU64::new(1),
        }));
    }
}

pub(crate) fn configure(backend: &mut d3d::GfxBackend) {
    if enabled() {
        let mut bits = None;
        backend.observe_recorded_pass(move |ctx, enc, source, target, clear, draws| {
            batch(ctx, enc, source, target, clear, draws, &mut bits);
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
        9 => Some([mem.u32(0x0050_9384).ok()?, surface]),
        10 => Some([surface, 0]),
        _ => None,
    }
}

impl<B: d3d::backend::Backend + 'static> Host for Glue<B> {
    fn begin(&self, mem: &mut dyn GuestMemory, layer: Layer) {
        let frame = crate::d3d_diag::presents() + 1;
        let Ok(mut d) = self.device.lock() else {
            return;
        };
        let bound = d.bound_target();
        let expected = if layer.layer == 10 {
            expected_targets(mem, layer.target)
        } else {
            None
        };
        let mem = DeviceMemory(mem);
        let (colour, depth) = (format(&mem, bound[0]), format(&mem, bound[1]));
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let tag = format!(
            "native-shadows frame={frame} seq={seq} {:?} layer={}",
            layer.site, layer.layer
        );
        eprintln!(
            "{tag} target={} bound={bound:x?} formats={colour:?}/{depth:?} flags={:#x} candidates={} projection={:08x?}",
            layer.target,
            layer.flags,
            layer.items.len(),
            layer.projection
        );
        for item in &layer.items {
            let choice = match item.geometry {
                shadows::Geometry::Other { .. } => None,
                geometry if layer.layer == 10 => shadows::generation(
                    layer.flags,
                    matches!(geometry, shadows::Geometry::Model { .. }),
                    item.marked,
                ),
                _ => None,
            };
            eprintln!(
                "{tag} item={} {:?} arg={:#x} shader={:#x} variant={} generate={choice:?}",
                item.index, item.geometry, item.argument, item.shader, item.variant
            );
        }
        // These are the inherited states before the material sets its own.
        let stencil = d.stencil_state(&mem);
        let read = |slot| d.render_state(&mem, slot);
        use d3d::state::slot;
        eprintln!(
            "{tag} begin stencil={stencil:?} shadowfunc={:?} bias_enable={:?} bias_bits={:x?} slope_bits={:x?}",
            read(slot::SHADOWFUNC),
            read(slot::SOLID_OFFSET_ENABLE),
            read(slot::POLYGON_OFFSET_ZOFFSET),
            read(slot::POLYGON_OFFSET_ZSLOPESCALE)
        );
        if layer.layer != 10 {
            return;
        }
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        *active = None;
        let kind = if shadows::colour_mask(layer.flags) {
            if !matches!(
                colour,
                Some(gfx::D3DFormat::A8R8G8B8 | gfx::D3DFormat::LinA8R8G8B8)
            ) {
                return;
            }
            MapKind::ColourMask
        } else {
            if !matches!(
                depth,
                Some(gfx::D3DFormat::D24S8 | gfx::D3DFormat::LinD24S8)
            ) {
                return;
            }
            MapKind::Depth
        };
        if expected != Some([bound[0], bound[1]]) || !d.can_external_pass() {
            eprintln!("{tag} refused: caster attachment identity");
            return;
        }
        let scope = Arc::new(Scope {
            seq,
            site: layer.site,
            kind,
            frame,
            bound,
        });
        d.request_external_pass(Box::new(Marker {
            scope: Arc::clone(&scope),
            begin: true,
        }));
        *active = Some(scope);
    }

    fn end(&self, _mem: &mut dyn GuestMemory, site: Site, layer: u32) {
        if layer != 10 {
            return;
        }
        let Ok(mut d) = self.device.lock() else {
            return;
        };
        let scope = self.active.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(scope) = scope
            && scope.site == site
            && scope.bound == d.bound_target()
        {
            d.request_external_pass(Box::new(Marker {
                scope,
                begin: false,
            }));
        }
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
        let label = format!(
            "frame={} seq={} {:?} {:?} packets={draws}",
            s.frame, s.seq, s.site, s.kind
        );
        compare(&ctx.device, enc, map.view.texture(), &reference, label)
    }
}

fn batch(
    ctx: &GfxContext,
    enc: &mut wgpu::CommandEncoder,
    source: &wgpu::Texture,
    target: &RenderTarget<'_>,
    clear: Clear,
    draws: &[RecordedDraw],
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
    label: String,
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
            eprintln!("native-shadows {label} readback failed");
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
                    first.get_or_insert((x, y));
                }
            }
        }
        eprintln!("native-shadows {label} map={w}x{h} texels_differ={differences} first={first:?}");
    });
    true
}
