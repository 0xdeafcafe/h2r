//! One resolved range at a shadow-layer boundary. The handoff owns the
//! original packet until this recording is complete on every live copy.
use d3d::backend::{ExternalAttachments, ExternalDraws, ExternalPassRequest};
use d3d::{ExternalTarget, ResolvedDraw, ShaderProvider};
use gfx::renderer::RenderScale;
use gfx::{Draw, GfxContext, RenderTarget, Renderer, VertexStream};
use native_render::shadow_gpu::{MapKind, SampledMap};
use std::sync::{Arc, Mutex};

/// The maps one caster call froze, one per target copy: a scaled backend
/// records a reference copy and a display copy of different sizes, and a
/// receiver binds the map of the copy it records on.
#[derive(Default)]
pub struct Maps(Vec<SampledMap>);

impl Maps {
    pub fn of(map: Option<SampledMap>) -> Self {
        Self(map.into_iter().collect())
    }

    /// Replace the copy's map. A failed capture (`None`) leaves that copy
    /// with none; `size` is the copy's target size.
    pub fn put(&mut self, size: (u32, u32), map: Option<SampledMap>) {
        let size = map.as_ref().map_or(size, |m| m.size);
        self.0.retain(|m| m.size != size);
        self.0.extend(map);
    }

    /// The map whose texture is `size` texels.
    pub fn at(&self, size: (u32, u32)) -> Option<&SampledMap> {
        self.0.iter().find(|m| m.size == size)
    }

    /// The last map put, for a consumer that records on one copy.
    pub fn latest(&self) -> Option<&SampledMap> {
        self.0.last()
    }
}

/// A per-caster-call value. Only this call's receiver and freeze marker
/// hold it. A failed capture leaves its copy empty.
pub type Output = Arc<Mutex<Maps>>;
pub struct Gpu {
    ctx: GfxContext,
    renderer: Renderer,
    shaders: d3d::Nv2aShaders,
    bits: gfx::depth_bits::DepthBits,
    filter: native_render::glow::QuadDrawer,
    defer_uploads: bool,
}

pub type SharedGpu = Arc<Mutex<Gpu>>;

/// The stage's device state. Its passes record on any target copy, at any
/// render scale: sizes come from the copy they are given.
fn new(ctx: GfxContext, defer_uploads: bool) -> SharedGpu {
    let mut renderer = Renderer::new(&ctx);
    renderer.set_staged_uploads(false);
    let bits = gfx::depth_bits::DepthBits::new(&ctx);
    Arc::new(Mutex::new(Gpu {
        ctx,
        renderer,
        shaders: d3d::Nv2aShaders::new(),
        bits,
        filter: native_render::glow::QuadDrawer::new(),
        defer_uploads,
    }))
}

pub fn for_backend(backend: &mut d3d::GfxBackend) -> SharedGpu {
    let gpu = new(crate::d3d_glue::share_context(&backend.ctx), true);
    let uploads = gpu.clone();
    backend.flush_external_uploads_with(move || {
        uploads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .flush_uploads();
    });
    let recycle = gpu.clone();
    backend.observe_submitted_frames(move || {
        recycle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .presented();
    });
    gpu
}

impl Gpu {
    pub fn filter(&mut self) -> &mut native_render::glow::QuadDrawer {
        &mut self.filter
    }

    /// The shadow stage's device, for passes that record on their own.
    pub fn context(&self) -> GfxContext {
        self.ctx.clone()
    }

    fn flush_uploads(&mut self) {
        self.renderer.flush_uploads(&self.ctx.queue);
        self.shaders.flush_uploads(&self.ctx);
    }

    /// Invoked from the physical backend's present hook, after its flush.
    pub fn presented(&mut self) {
        self.renderer.submitted();
        self.shaders.submitted();
        self.shaders.end_frame();
        let _ = self.renderer.take_new_keys();
    }
}

pub struct Freeze {
    pub gpu: SharedGpu,
    pub output: Output,
    pub kind: MapKind,
    pub sampled: Option<u32>,
}

impl d3d::ExternalPass for Freeze {
    // Reads back what it draws: a kept scene cannot record it again.
    fn replays_scene(&self) -> bool {
        false
    }

    fn colour_only(&self) -> bool {
        self.kind == MapKind::ColourMask
    }
    fn sampled(&self) -> Vec<u32> {
        self.sampled.into_iter().collect()
    }
    fn encode(&mut self, t: ExternalTarget<'_>) {
        let gpu = self.gpu.lock().unwrap_or_else(|e| e.into_inner());
        if t.device != &gpu.ctx.device {
            return;
        }
        let source = match self.kind {
            MapKind::Depth => t.depth.texture(),
            MapKind::ColourMask => {
                let [Some(level)] = t.sampled else { return };
                level.view.texture()
            }
        };
        // Snapshot the actual result after all native and D3D casters.
        let map = SampledMap::snapshot(&gpu.ctx, t.encoder, &gpu.bits, self.kind, source);
        self.output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .put(t.size, map);
    }
}

/// A scissor in guest pixels, clamped to the guest surface and scaled to
/// the copy's texels.
fn scale_rect(grid: RenderScale, guest: (u32, u32), r: d3d::PixelRect) -> [u32; 4] {
    let x = r.x.min(guest.0);
    let y = r.y.min(guest.1);
    grid.rect(x, y, r.width.min(guest.0 - x), r.height.min(guest.1 - y))
}

pub struct Range {
    pub gpu: SharedGpu,
    pub draw: Arc<ResolvedDraw>,
    pub map: Option<(Output, MapKind, u32)>,
}

impl Range {
    fn prepare(
        &self,
        device: &wgpu::Device,
        copy: d3d::TargetCopy,
        guest_size: (u32, u32),
        target: RenderTarget<'_>,
        stages: [Option<&wgpu::TextureView>; 4],
    ) -> Option<(Vec<gfx::RecordedDraw>, bool)> {
        let mut gpu = self.gpu.lock().ok()?;
        let Gpu {
            ctx,
            renderer,
            shaders,
            defer_uploads,
            ..
        } = &mut *gpu;
        let draw = self.draw.for_copy(copy);
        let draw = draw.as_ref();
        // The copy's grid over the guest's: the identity at 1x.
        let grid = RenderScale::between(guest_size, target.size);
        if &ctx.device != device
            || grid.is_none()
            || guest_size != (draw.target.width, draw.target.height)
            || (draw.state.render.stencil.enable
                && !target
                    .depth_format
                    .is_some_and(|format| format.has_stencil_aspect()))
        {
            return None;
        }
        let grid = grid?;
        let maps = self.map.as_ref().and_then(|(out, _, _)| out.lock().ok());
        // Shortened to the map guard's lifetime: a stage may sample its view.
        let mut stages: [Option<&wgpu::TextureView>; 4] = stages;
        for (i, info) in draw.texture_info.iter().enumerate() {
            if info.is_some() != stages[i].is_some() {
                return None;
            }
            if let (Some(info), Some((_, kind, header))) = (info, &self.map) {
                if info.header == *header {
                    let bound = stages[i]?.texture();
                    let source = maps.as_deref()?.at((bound.width(), bound.height()))?;
                    let guest_format = match kind {
                        MapKind::Depth => matches!(
                            info.format,
                            gfx::D3DFormat::D24S8 | gfx::D3DFormat::LinD24S8
                        ),
                        MapKind::ColourMask => matches!(
                            info.format,
                            gfx::D3DFormat::A8R8G8B8 | gfx::D3DFormat::LinA8R8G8B8
                        ),
                    };
                    if !guest_format
                        || info.kind != gfx::texture::TextureKind::D2
                        || info.levels != 1
                        || source.kind != *kind
                        || (source.size != (info.width, info.height)
                            && source.size != grid.size(info.width, info.height))
                    {
                        return None;
                    }
                    stages[i] = Some(&source.view);
                }
            }
        }
        if stages.iter().flatten().any(|view| {
            view.texture() == target.color.texture()
                || target
                    .depth
                    .is_some_and(|depth| view.texture() == depth.texture())
                || !view
                    .texture()
                    .usage()
                    .contains(wgpu::TextureUsages::TEXTURE_BINDING)
                || !gfx::program::filterable(view.texture().format(), device.features())
        }) {
            return None;
        }
        let prepared = shaders.prepare(ctx, renderer, draw, stages).ok()?;
        let mut state = draw.state.render;
        if prepared.writes_depth {
            state.solid_offset_enable = false;
            state.point_offset_enable = false;
            state.wireframe_offset_enable = false;
        }
        let rects = if draw.clip_rects.is_empty() {
            draw.clip.as_slice()
        } else {
            &draw.clip_rects
        };
        if rects.is_empty() {
            return None;
        }
        let streams: Vec<_> = draw
            .streams
            .iter()
            .map(|s| VertexStream {
                data: &s.data,
                stride: s.stride,
            })
            .collect();
        let ids: Vec<_> = draw.streams.iter().map(|s| s.data.cache_id()).collect();
        let mut d = Draw {
            bind_groups: &prepared.bind_groups,
            vertex_fetch: renderer.vertex_fetch(),
            indices: draw.indices.as_deref(),
            stream_ids: &ids,
            index_id: draw.indices.as_ref().and_then(|v| v.cache_id()),
            ..Draw::new(
                prepared.id,
                &draw.layout,
                &streams,
                draw.vertex_count,
                draw.primitive,
                state,
            )
        };
        let mut packets = Vec::with_capacity(rects.len());
        for &rect in rects {
            let [x, y, width, height] = scale_rect(grid, guest_size, draw.display_rect(rect));
            d.scissor = Some(gfx::renderer::ScissorRect {
                x,
                y,
                width,
                height,
            });
            let packet = renderer.record(ctx, &target, &d).ok()?;
            if !packet.pipeline_valid() {
                return None;
            }
            packets.push(packet);
        }
        if !*defer_uploads {
            gpu.flush_uploads();
        }
        Some((packets, prepared.discards))
    }
}

impl d3d::ExternalPass for Range {
    fn colour_only(&self) -> bool {
        self.draw.target.depth_format.is_none()
    }
    fn writes(&self) -> d3d::ExternalWrites {
        let r = self.draw.state.render;
        let f = self.draw.target.depth_format;
        let writes_z = r.depth_write && r.depth_mode != gfx::state::DepthBufferMode::Disabled;
        let s = r.stencil;
        let writes_s = s.enable
            && s.write_mask != 0
            && [s.fail, s.depth_fail, s.pass]
                .iter()
                .any(|&op| op != gfx::state::StencilOp::Keep)
            && matches!(
                f,
                Some(
                    gfx::D3DFormat::D24S8
                        | gfx::D3DFormat::LinD24S8
                        | gfx::D3DFormat::F24S8
                        | gfx::D3DFormat::LinF24S8
                )
            );
        d3d::ExternalWrites {
            color: true,
            depth_stencil: f.is_some() && (writes_z || writes_s),
        }
    }
    fn resolved_fallback(&self) -> Option<Arc<ResolvedDraw>> {
        Some(Arc::clone(&self.draw))
    }

    fn record_draws(
        &mut self,
        t: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        let has_depth = self.draw.target.depth_format.is_some();
        let target = RenderTarget {
            color: &t.color,
            color_format: t.color_format,
            depth: has_depth.then_some(&t.depth),
            depth_format: has_depth.then_some(t.depth_format),
            size: t.size,
            samples: 1,
        };
        let Some((packets, discards)) = self.prepare(
            &t.device,
            t.copy,
            request.surface_size,
            target,
            t.stages.each_ref().map(Option::as_ref),
        ) else {
            return Some(Err("shadow resources or shader unavailable"));
        };
        Some(Ok(ExternalDraws {
            ranges: packets.len(),
            packets,
            discards,
            writes: Vec::new(),
        }))
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    fn map(ctx: &GfxContext, size: (u32, u32)) -> SampledMap {
        let view = ctx
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("scaled map"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        SampledMap {
            kind: MapKind::ColourMask,
            view,
            size,
        }
    }

    #[test]
    fn each_copy_of_a_scaled_view_keeps_its_own_map() {
        let Ok(ctx) = GfxContext::headless() else {
            return;
        };
        let mut maps = Maps::default();
        maps.put((256, 256), Some(map(&ctx, (256, 256))));
        maps.put((512, 512), Some(map(&ctx, (512, 512))));
        assert_eq!(maps.at((256, 256)).unwrap().size, (256, 256));
        assert_eq!(maps.at((512, 512)).unwrap().size, (512, 512));
        assert_eq!(maps.latest().unwrap().size, (512, 512));
        assert!(maps.at((128, 128)).is_none());
        // A failed capture on one copy leaves only that copy without a map.
        maps.put((512, 512), None);
        assert!(maps.at((512, 512)).is_none());
        assert!(maps.at((256, 256)).is_some());
    }

    #[test]
    fn a_scissor_scales_to_the_copy_and_stays_inside_the_guest_surface() {
        let guest = (640, 480);
        let at = |scale: u32, r: d3d::PixelRect| {
            scale_rect(RenderScale::between(guest, (640 * scale, 480 * scale)).unwrap(), guest, r)
        };
        let rect = d3d::PixelRect {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        assert_eq!(at(1, rect), [10, 20, 100, 50]);
        assert_eq!(at(2, rect), [20, 40, 200, 100]);
        assert_eq!(at(3, rect), [30, 60, 300, 150]);
        let wide = d3d::PixelRect {
            x: 600,
            y: 400,
            width: 400,
            height: 400,
        };
        assert_eq!(at(2, wide), [1200, 800, 80, 160]);
    }
}

#[cfg(test)]
#[path = "native_shadows_tests.rs"]
mod tests;
