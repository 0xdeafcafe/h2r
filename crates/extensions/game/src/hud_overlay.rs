//! Individual HUD draws after the guest has installed the widget shader.
//! The draw snapshot pins the sampled image, including the changing glyph cache.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use d3d::backend::{ExternalAttachments, ExternalEncoded};
use d3d::{DrawAdmission, DrawSnapshot, ExternalPass, ExternalPassRequest, ExternalPlan};
use engine::memory::GuestMemory;
use frame_scene::{
    ClipRect, Overlay, OverlayBlend, OverlayMaterial, OverlayTexture, OverlayVertex, Program,
    Sampler,
};
use gfx::state::{
    BlendFactor, CompareFunc, CullMode, DepthBufferMode, FillMode, TextureAddress, TextureFilter,
};
use native_render::overlay::{NoGlyphs, OverlayPlan};
use native_render::ui_gpu::{OverlayGpu, OverlayTextures};
use render_bridge::native_hud::{HudTagQuad, HudVertices};
use timeline::native::Stage;

use crate::native_lights_glue::DeviceMemory;

pub fn draw_tag_quad<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    at: &HudTagQuad,
    vertices: &HudVertices,
) -> bool {
    submit(device, mem, at, vertices, false)
}

pub fn draw_text_quad<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    at: &HudTagQuad,
    vertices: &HudVertices,
) -> bool {
    submit(device, mem, at, vertices, true)
}

fn submit<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    at: &HudTagQuad,
    vertices: &HudVertices,
    glyph: bool,
) -> bool {
    let admission = Widget {
        at,
        glyph,
        captured: Mutex::new(None),
    };
    let Ok(mut device) = device.lock() else {
        return false;
    };
    let mut mem = DeviceMemory(mem);
    let fan = gfx::vertex::PrimitiveType::TriangleFan;
    let taken = if glyph {
        device.try_native_pushed_draw(&mut mem, fan, vertices, &admission)
    } else {
        device.try_native_immediate_draw(&mut mem, fan, vertices, &admission)
    };
    drop(device);
    if taken.ok() != Some(true) {
        return false;
    }
    if let Some(overlay) = admission
        .captured
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
    {
        let _ = render_bridge::scene_step::with_overlay(|o| o.append(&overlay));
    }
    true
}

struct Widget<'a> {
    at: &'a HudTagQuad,
    glyph: bool,
    captured: Mutex<Option<Arc<Overlay>>>,
}

fn sampler(draw: &d3d::ResolvedDraw, stage: usize) -> Option<Sampler> {
    let s = &draw.state.stages[stage];
    let extra = &draw.extras[stage];
    let info = draw.texture_info[stage]?;
    if info.kind != gfx::texture::TextureKind::D2
        || info.format.is_depth()
        || extra.color_sign != 0
        || extra.color_key_op != 0
        || [s.address_u, s.address_v].contains(&TextureAddress::Border)
        || (info.levels > 1 && s.mip_lod_bias != 0.0)
    {
        return None;
    }
    // Basic sampler fields remain available to scene inspection. At recording
    // the resolved stage supplies all three address modes, filters and mip levels.
    Some(Sampler {
        wrap: s.address_u == TextureAddress::Wrap,
        point: s.min_filter == TextureFilter::Point,
    })
}

fn blend(rs: &gfx::state::RenderState) -> Option<OverlayBlend> {
    if !rs.blend_enable {
        return Some(OverlayBlend::Off);
    }
    let constant = |v| {
        matches!(
            v,
            BlendFactor::ConstantColor
                | BlendFactor::InvConstantColor
                | BlendFactor::ConstantAlpha
                | BlendFactor::InvConstantAlpha
        )
    };
    if constant(rs.src_blend) || constant(rs.dst_blend) {
        return None;
    }
    Some(OverlayBlend::On {
        source: rs.src_blend.xbox_value(),
        dest: rs.dst_blend.xbox_value(),
        equation: rs.blend_op.xbox_value(),
    })
}

fn clip(r: d3d::backend::PixelRect) -> ClipRect {
    ClipRect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

fn faces_visible(corners: &[OverlayVertex; 4], cull: CullMode) -> bool {
    if cull == CullMode::None {
        return true;
    }
    let [a, b, c, d] = corners.map(|v| v.position);
    let visible = |p: [f32; 2], q: [f32; 2]| {
        let cross = (p[0] - a[0]) * (q[1] - a[1]) - (p[1] - a[1]) * (q[0] - a[0]);
        match cull {
            CullMode::Cw => cross < 0.0,
            _ => cross > 0.0,
        }
    };
    visible(b, c) && visible(c, d)
}

impl DrawAdmission for Widget<'_> {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let draw = snapshot.draw;
        let rs = &draw.state.render;
        if snapshot.color_header == 0
            || !draw.clip_rects.is_empty()
            || draw.writes_depth_stencil()
            || (rs.depth_mode != DepthBufferMode::Disabled && rs.depth_func != CompareFunc::Always)
            || rs.stencil.enable
            || rs.fill_mode != FillMode::Solid
            || draw.hw.zpass_count_enable
            || rs.color_write.xbox_value() == 0
        {
            return None;
        }
        let words = d3d::nv2a::overlay_shader_words(draw)?;
        if !native_render::ui_gpu::hud_shader_supported(&words) {
            return None;
        }
        let sampled = snapshot.sampled_stages;
        let first = sampled.iter().position(|&v| v);
        let mut shader_stages = [None; 4];
        let mut samplers = [Sampler::default(); 4];
        let mut alpha_kill = 0;
        for (stage, &active) in sampled.iter().enumerate() {
            if !active {
                continue;
            }
            samplers[stage] = sampler(draw, stage)?;
            let header = snapshot.bound_textures[stage];
            if header == 0 {
                return None;
            }
            shader_stages[stage] = Some(OverlayTexture::Bound { header });
            if draw.extras[stage].alpha_kill & 4 != 0 {
                alpha_kill |= 1u8 << stage;
            }
        }
        if self.glyph
            && !shader_stages.contains(&Some(OverlayTexture::Bound {
                header: self.at.bitmap,
            }))
        {
            return None;
        }
        let texture = match first {
            Some(n) if self.glyph => shader_stages[n]?,
            Some(_) => OverlayTexture::Bitmap {
                tag: self.at.bitmap,
                image: self.at.image,
            },
            None => OverlayTexture::None,
        };
        let evaluated = d3d::nv2a::overlay_vertices(draw, sampled)?;
        let corners = evaluated.map(|(position, coords, color)| OverlayVertex {
            position,
            uv: coords[first.unwrap_or(0)],
            color,
        });
        if !faces_visible(&corners, rs.cull_mode) {
            return None;
        }
        let material = OverlayMaterial {
            program: Program::Shader {
                tag: self.at.shader,
            },
            texture,
            sampler: first.map_or_else(Sampler::default, |n| samplers[n]),
            blend: blend(rs)?,
        };
        let mut overlay = Overlay::new([draw.target.width, draw.target.height]);
        overlay
            .push_quad(material, clip(draw.clip?), corners)
            .ok()?;
        let item = &mut overlay.items[0];
        item.alpha_test = rs
            .alpha_test_enable
            .then_some((rs.alpha_func.xbox_value(), rs.alpha_ref));
        item.alpha_kill = alpha_kill;
        item.shader_stages = shader_stages;
        item.stage_uv = Some(Box::new(evaluated.map(|(_, coords, _)| coords)));
        item.glyph_cache = draw.glyph_cache;
        item.hud_input = self.at.hud_input;
        item.shader_words = Some(Box::new(words));
        item.color_write = rs.color_write.xbox_value();
        let overlay = Arc::new(overlay);
        *self.captured.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&overlay));
        let pass = WidgetPass {
            draw: Arc::new(draw.clone()),
            overlay,
            sampled,
        };
        Some((
            ExternalPlan {
                stage: Stage::Ui,
                view: 4,
                ranges: 1,
                slots: vec![],
            },
            Box::new(pass),
        ))
    }
}

struct WidgetPass {
    draw: Arc<d3d::ResolvedDraw>,
    overlay: Arc<Overlay>,
    sampled: [bool; 4],
}

struct Bound<'a> {
    views: &'a [Option<wgpu::TextureView>; 4],
    samplers: [Option<wgpu::Sampler>; 4],
    first: Option<usize>,
}
impl OverlayTextures for Bound<'_> {
    fn view(&self, _: OverlayTexture) -> Option<&wgpu::TextureView> {
        self.views[self.first?].as_ref()
    }
    fn stage(&self, stage: usize, _: OverlayTexture) -> Option<&wgpu::TextureView> {
        self.views.get(stage)?.as_ref()
    }
    fn stage_sampler(&self, stage: usize, _: OverlayTexture) -> Option<&wgpu::Sampler> {
        self.samplers.get(stage)?.as_ref()
    }
}

struct Renderer {
    device: wgpu::Device,
    gpu: OverlayGpu,
    samplers: HashMap<gfx::translate::SamplerKey, wgpu::Sampler>,
}
static GPU: Mutex<Option<Renderer>> = Mutex::new(None);

/// Same window-coordinate fixup as the vertex translator in vsh/wgsl.rs.
fn raster_position(window: f32, size: u32, shift: f32) -> f32 {
    if shift == 0.0 {
        return (window * 16.0).trunc() / 16.0;
    }
    let pos = window - shift;
    if window > size as f32 - 0.5 {
        pos.max(size as f32)
    } else {
        pos
    }
}

impl WidgetPass {
    fn record(
        &self,
        t: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Result<ExternalEncoded, &'static str> {
        for (sampled, view) in self.sampled.iter().zip(&t.stages) {
            if *sampled && view.as_ref().is_some_and(|v| {
                v.texture() == t.color.texture() || v.texture() == t.depth.texture()
            }) {
                return Err("HUD texture aliases its render target");
            }
        }
        let mut overlay = (*self.overlay).clone();
        let (w, h) = request.surface_size;
        if overlay.size != [w, h] {
            return Err("HUD surface size");
        }
        let scale =
            gfx::raster::RenderScale::between(request.surface_size, t.size).ok_or("HUD scale")?;
        let draw = self.draw.for_copy(t.copy);
        let transform = draw.ui_fit.map_or([1., 1., 0., 0.], |f| f.transform);
        let shift = scale.shift();
        for v in &mut overlay.vertices {
            for i in 0..2 {
                let p = v.position[i] * transform[i] + transform[i + 2];
                v.position[i] = raster_position(p, overlay.size[i], shift[i]);
            }
        }
        overlay.items[0].clip = clip(draw.display_rect(draw.clip.ok_or("HUD clip")?));
        let plan = OverlayPlan::new(&overlay, [t.size.0, t.size.1], &NoGlyphs);
        let mut lock = GPU.lock().unwrap_or_else(|e| e.into_inner());
        if lock
            .as_ref()
            .is_none_or(|renderer| renderer.device != t.device)
        {
            *lock = Some(Renderer {
                device: t.device.clone(),
                gpu: OverlayGpu::new(&t.device, &t.queue),
                samplers: HashMap::new(),
            });
        }
        let renderer = lock.as_mut().ok_or("HUD device")?;
        let caps = gfx::translate::Capabilities::from_features(t.device.features());
        let samplers = std::array::from_fn(|n| {
            self.sampled[n].then(|| {
                let key = gfx::translate::SamplerKey::from_stage(&draw.state.stages[n], caps);
                renderer
                    .samplers
                    .entry(key)
                    .or_insert_with(|| t.device.create_sampler(&key.descriptor()))
                    .clone()
            })
        });
        let textures = Bound {
            views: &t.stages,
            samplers,
            first: self.sampled.iter().position(|&v| v),
        };
        let (encoded, stats) = renderer
            .gpu
            .prepare(&t.device, &t.color, &plan, &textures)?;
        if encoded.ranges != 1 || stats.batches != 1 {
            return Err("HUD material refused");
        }
        if t.copy == d3d::TargetCopy::Display {
            native_render::ui_gpu::count_overlay(&stats);
        }
        Ok(encoded)
    }
}

impl ExternalPass for WidgetPass {
    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        Some(Arc::clone(&self.draw))
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: true,
            depth_stencil: false,
        }
    }

    fn colour_only(&self) -> bool {
        true
    }

    fn record_encoded(
        &mut self,
        t: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalEncoded, &'static str>> {
        let result = self.record(t, request);
        if result.is_err() {
            timeline::native::Ui::OverlayHudTagRecordFailed.count();
        } else if t.copy == d3d::TargetCopy::Display {
            timeline::native::Ui::OverlayHudTagRecorded.count();
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scaled_raster_keeps_far_edges_and_reference_snaps_to_sixteenths() {
        assert_eq!(raster_position(3.09375, 8, 0.0), 3.0625);
        assert_eq!(raster_position(3.0, 8, 0.375), 2.625);
        assert_eq!(raster_position(7.75, 8, 0.375), 8.0);
    }
    #[test]
    fn both_triangles_of_a_widget_must_survive_culling() {
        let quad = [[1., 1.], [5., 1.], [5., 3.], [1., 3.]].map(|position| OverlayVertex {
            position,
            ..Default::default()
        });
        assert!(faces_visible(&quad, CullMode::Ccw));
        assert!(!faces_visible(&quad, CullMode::Cw));
        let mut crossed = quad;
        crossed.swap(2, 3);
        assert!(!faces_visible(&crossed, CullMode::Ccw));
    }
}
