//! One resolved range at a shadow-layer boundary. The handoff owns the
//! original packet until this recording is complete on every live copy.
use d3d::{ExternalTarget, ResolvedDraw, ShaderProvider};
use gfx::{Clear, Draw, GfxContext, RenderTarget, Renderer, VertexStream};
use native_render::shadow_gpu::{MapKind, SampledMap};
use std::sync::{Arc, Mutex};

/// A per-caster-call value. Only this call's receiver and freeze marker
/// hold it. A failed capture leaves it empty.
pub(crate) type Output = Arc<Mutex<Option<SampledMap>>>;
pub(crate) struct Gpu {
    ctx: GfxContext,
    renderer: Renderer,
    shaders: d3d::Nv2aShaders,
    bits: gfx::depth_bits::DepthBits,
    defer_uploads: bool,
}

pub(crate) type SharedGpu = Arc<Mutex<Gpu>>;

/// Called only for a single, retail, 1x backend. The scaled backend has
/// a separate E3 caster target that a generic external pass cannot name.
fn new(ctx: GfxContext, defer_uploads: bool) -> SharedGpu {
    let mut renderer = Renderer::new(&ctx);
    renderer.set_staged_uploads(false);
    let bits = gfx::depth_bits::DepthBits::new(&ctx);
    Arc::new(Mutex::new(Gpu {
        ctx,
        renderer,
        shaders: d3d::Nv2aShaders::new(),
        bits,
        defer_uploads,
    }))
}

pub(crate) fn for_backend(backend: &mut d3d::GfxBackend) -> SharedGpu {
    let gpu = new(crate::d3d_glue::share_context(&backend.ctx), true);
    let uploads = gpu.clone();
    backend.flush_external_uploads_with(move || {
        uploads.lock().unwrap_or_else(|e| e.into_inner()).flush_uploads();
    });
    let recycle = gpu.clone();
    backend.observe_submitted_frames(move || {
        recycle.lock().unwrap_or_else(|e| e.into_inner()).presented();
    });
    gpu
}

impl Gpu {
    fn flush_uploads(&mut self) {
        self.renderer.flush_uploads(&self.ctx.queue);
        self.shaders.flush_uploads(&self.ctx);
    }

    /// Invoked from the physical backend's present hook, after its flush.
    pub(crate) fn presented(&mut self) {
        self.renderer.submitted();
        self.shaders.submitted();
        self.shaders.end_frame();
        let _ = self.renderer.take_new_keys();
    }
}

pub(crate) struct Freeze {
    pub(crate) gpu: SharedGpu,
    pub(crate) output: Output,
    pub(crate) kind: MapKind,
    pub(crate) sampled: Option<u32>,
}

impl d3d::ExternalPass for Freeze {
    fn colour_only(&self) -> bool {
        self.kind == MapKind::ColourMask
    }
    fn sampled(&self) -> Vec<u32> {
        self.sampled.into_iter().collect()
    }
    fn encode(&mut self, t: ExternalTarget<'_>) {
        let gpu = self.gpu.lock().unwrap_or_else(|e| e.into_inner());
        if t.device != &gpu.ctx.device || t.size != t.guest_size {
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
        *self.output.lock().unwrap_or_else(|e| e.into_inner()) = map;
    }
}

pub(crate) struct Range {
    pub(crate) gpu: SharedGpu,
    pub(crate) draw: Arc<ResolvedDraw>,
    pub(crate) map: Option<(Output, MapKind, u32)>,
    pub(crate) packets: Vec<gfx::RecordedDraw>,
    pub(crate) recorded: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

impl Range {
    fn target<'a>(&self, t: &ExternalTarget<'a>) -> RenderTarget<'a> {
        RenderTarget {
            color: t.color,
            color_format: t.color_format,
            depth: self.draw.target.depth_format.map(|_| t.depth),
            depth_format: self.draw.target.depth_format.map(|_| t.depth_format),
            size: t.size,
            samples: 1,
        }
    }
    fn prepare(&self, t: &ExternalTarget<'_>) -> Option<Vec<gfx::RecordedDraw>> {
        let mut gpu = self.gpu.lock().ok()?;
        let Gpu {
            ctx,
            renderer,
            shaders,
            defer_uploads,
            ..
        } = &mut *gpu;
        let draw = &self.draw;
        let target = self.target(t);
        if &ctx.device != t.device
            || t.copy != d3d::TargetCopy::Reference
            || t.size != t.guest_size
            || t.size != (draw.target.width, draw.target.height)
        {
            return None;
        }
        let map = self.map.as_ref().and_then(|(out, _, _)| out.lock().ok());
        let mut stages = t.stages;
        for (i, info) in draw.texture_info.iter().enumerate() {
            if info.is_some() != stages[i].is_some() {
                return None;
            }
            if let (Some(info), Some((_, kind, header))) = (info, &self.map) {
                if info.header == *header {
                    let source = map.as_deref()?.as_ref()?;
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
                    let bound = stages[i]?.texture();
                    if !guest_format
                        || info.kind != gfx::texture::TextureKind::D2
                        || info.levels != 1
                        || source.kind != *kind
                        || source.size != (info.width, info.height)
                        || source.size != (bound.width(), bound.height())
                    {
                        return None;
                    }
                    stages[i] = Some(&source.view);
                }
            }
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
        for r in rects {
            d.scissor = Some(gfx::renderer::ScissorRect {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            });
            packets.push(renderer.record(ctx, &target, &d).ok()?);
        }
        if !*defer_uploads {
            gpu.flush_uploads();
        }
        Some(packets)
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
        // The layer-end probe reads the recorded count on the guest thread.
        self.recorded.is_none().then(|| Arc::clone(&self.draw))
    }

    fn prepare_copy(&mut self, t: &ExternalTarget<'_>) -> Result<(), &'static str> {
        self.packets = self
            .prepare(t)
            .ok_or("shadow resources or shader unavailable")?;
        Ok(())
    }
    fn record_ranges(&mut self, t: ExternalTarget<'_>) -> usize {
        let Ok(gpu) = self.gpu.lock() else { return 0 };
        let target = self.target(&t);
        if Renderer::encode_pass(
            &gpu.ctx,
            t.encoder,
            &target,
            Clear::NONE,
            &self.packets,
            None,
        )
        .is_ok()
        {
            if let Some(count) = &self.recorded {
                count.fetch_add(self.packets.len(), std::sync::atomic::Ordering::Relaxed);
            }
            self.packets.len()
        } else {
            0
        }
    }
}

#[cfg(test)]
#[path = "native_shadows_tests.rs"]
mod tests;
