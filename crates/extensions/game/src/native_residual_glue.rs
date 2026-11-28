//! Admission of sorted pool surfaces and sky strips. Layer 2's standard
//! geometry draws from the plan (`native_layers_glue`), what it keeps on
//! the guest (docs/graphics/retire-d3d-r5.md).

use native_render::world_layers_gpu::IndexedPass;
use render_bridge::native_sky;

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let enabled = hooks::Switches::from_env().is_ok_and(|s| hooks::switches::native_render_on(&s));
    if enabled && let Ok(mut device) = device.lock() {
        device.set_native_residual(true);
        device.add_draw_admission(std::sync::Arc::new(ResidualDraws::default()));
    }
    if enabled {
        crate::native_sky_cut_glue::install(device);
    }
}

/// The refusal counted (`layer_plan_15_pool_on_d3d`) for each sorted pool
/// draw left on the D3D path, besides its own cause
/// (`layer_plan_15_pool_refused_<cause>`).
pub(crate) const POOL_ON_D3D: &str = "pool_on_d3d";

/// A sorted pool issue `(view, index count)` as a native water pass, or the
/// guest's D3D draw with the cause counted.
fn admit_pool(
    draw: &d3d::ResolvedDraw, view: u32, count: u32, cache: std::sync::Arc<native_render::world_layers_gpu::Cache>,
) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
    use native_render::world_layers_gpu::pool_refusal_counter;
    let refuse = |cause: &str| {
        // The cause counter is `layer_plan_15_pool_refused_<cause>`.
        render_bridge::native_layers::count(15, pool_refusal_counter(cause).trim_start_matches("layer_plan_15_"), 1);
        render_bridge::native_layers::count(15, POOL_ON_D3D, 1);
        None
    };
    if draw.indices.as_deref().is_none_or(|i| i.len() != count as usize) {
        return refuse("index_count");
    }
    let frame = crate::d3d_diag::presents() + 1;
    match IndexedPass::for_pool(draw, cache, frame) {
        Ok(pass) => {
            render_bridge::native_layers::count(15, "pool_admitted", 1);
            Some((
                d3d::ExternalPlan { stage: timeline::native::Stage::Water, view, ranges: 1, slots: Vec::new() },
                Box::new(pass),
            ))
        }
        Err(cause) => refuse(cause),
    }
}

#[derive(Default)]
struct ResidualDraws {
    geometry: crate::native_map::MapIndexedCache,
}

impl d3d::DrawAdmission for ResidualDraws {
    fn admit(
        &self,
        snapshot: &d3d::DrawSnapshot<'_>,
    ) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        use native_render::world_layers::Site;

        let draw = snapshot.draw;
        if draw.entry != "D3DDevice_DrawIndexedVertices" {
            return None;
        }
        if let Some((view, count)) = crate::native_world_layers_glue::pool_issue() {
            return admit_pool(draw, view, count, self.geometry.get());
        }
        let call = crate::native_world_layers_glue::indexed_draw()?;
        if (call.layer.site, call.layer.number) != (Site::Sky, 15) {
            return None;
        }
        let issue = crate::native_world_layers_glue::take_sky()?;
        // The pass's program and textures from the material beside the
        // strip's, until the cut binds them.
        native_sky::count_plan(issue.look.as_ref().map_or_else(|why| *why, |look| look.check(draw)));
        // The section's geometry from the load beside the strip's; a strip
        // whose geometry matches draws its resident `SectionKey::Sky` entry
        // from the load's snapshots.
        let resident = issue.geometry.as_ref().map_err(|why| *why).and_then(|(section, part)| section.resident(*part, draw));
        native_sky::count_geometry(resident.as_ref().map_or_else(|why| *why, |_| "matched"));
        // The cut's arm: look and geometry both the plan's.
        native_sky::cut_outcome(call.generation, issue.look.as_ref().is_ok_and(|look| look.check(draw) == "matched")
            && resident.is_ok());
        let constants = issue.constants;
        let item = issue.item.as_ref().ok().cloned();
        let plan = native_sky::admit(
            issue.item, call.layer.view, call.index_count, draw.indices.as_ref().map(|i| i.len()),
            // The plan's evaluated material, or the cause it has none.
            |item| IndexedPass::for_sky(draw, self.geometry.get(), call.frame, call.generation, item, constants?, resident.ok()),
        );
        let (stage, pass) = match plan {
            Ok(pass) => {
                native_sky::count("admitted");
                // Kept for the cut to replay (`native_sky_cut_glue`).
                crate::native_sky_cut_glue::record(call.generation, item.as_ref().map(|item| (&pass, item)), draw);
                (timeline::native::Stage::Sky, pass)
            }
            Err(reason) => {
                native_sky::count(reason);
                crate::native_sky_cut_glue::record(call.generation, None, draw);
                return None;
            }
        };
        Some((
            d3d::ExternalPlan {
                stage,
                view: call.layer.view,
                ranges: 1,
                slots: Vec::new(),
            },
            Box::new(pass),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use d3d::backend::{StreamBytes, TargetInfo, Viewport};
    use d3d::shader::{ShaderInputs, VertexProcessing};
    use d3d::state::StateWords;
    use d3d::{DrawAdmission, DrawSnapshot};
    use gfx::vertex::{
        PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
    };

    use super::*;

    /// An indexed triangle the native indexed pass would translate.
    fn indexed_triangle() -> d3d::ResolvedDraw {
        let words = StateWords::defaults(false);
        let data: Vec<u8> = [[0i16, 0, 0, 0], [1, 0, 0, 0], [1, 1, 0, 0]]
            .into_iter()
            .flatten()
            .flat_map(i16::to_le_bytes)
            .collect();
        d3d::ResolvedDraw {
            entry: "D3DDevice_DrawIndexedVertices",
            primitive: PrimitiveType::TriangleList,
            vertex_count: 3,
            layout: VertexLayout {
                streams: vec![StreamLayout {
                    stream: 0,
                    stride: 8,
                    attributes: vec![VertexAttribute {
                        register: VertexRegister(0),
                        data_type: VertexDataType::Short4,
                        offset: 0,
                    }],
                }],
                pretransformed: false,
            },
            streams: vec![StreamBytes { data: data.into(), stride: 8 }],
            indices: Some(vec![0, 1, 2].into()),
            state: words.translate(&gfx::xdk::RenderStateTable::default()),
            extras: words.stage_extras(),
            raw: Arc::new(words),
            target: TargetInfo {
                width: 64,
                height: 64,
                format: gfx::D3DFormat::A8R8G8B8,
                depth_format: Some(gfx::D3DFormat::D24S8),
            },
            shader: ShaderInputs {
                vertex: VertexProcessing::Program { start: 0, microcode: Vec::<u32>::new().into() },
                provided_inputs: 1,
                vertex_defaults: [[0., 0., 0., 1.]; 16],
                constants: Arc::new([[0.; 4]; 192]),
                pixel_shader: Some(Arc::new([0; 60])),
                combiner_state: Arc::new([0; 57]),
                final_combiner_latch: true,
                affine_projection: false,
            },
            viewport: Viewport { x: 0, y: 0, width: 64, height: 64, min_z: 0., max_z: 1. },
            clip: Some(d3d::PixelRect { x: 0, y: 0, width: 64, height: 64 }),
            clip_rects: Vec::new(),
            hw: Default::default(),
            texture_info: [None; 4],
            textures: [None; 4],
            retail_constants: None,
            ui_fit: None,
            glyph_cache: 0,
        }
    }

    fn counter(name: &str) -> u64 {
        render_bridge::native_layers::counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, n)| n)
    }

    fn snapshot(draw: &d3d::ResolvedDraw) -> DrawSnapshot<'_> {
        DrawSnapshot {
            draw,
            vertex_shader: 0,
            pixel_shader: 0,
            bound_textures: [0; 4],
            sampled_stages: [false; 4],
            palettes: [0; 4],
            color_header: 1,
            depth_header: 2,
        }
    }

    /// The pool draws the test's triangle can become: a projective lens of
    /// the target's size at stage 0 that the device resolved as a frame
    /// sample, depth tested and never written.
    fn pool_draw() -> d3d::ResolvedDraw {
        use shaders::psh::{PixelShaderDef, bits::*};
        let mut draw = indexed_triangle();
        let def = PixelShaderDef {
            texture_modes: texture_modes([TEX_PROJECT2D, 0, 0, 0]),
            final_inputs_abcd: inputs(0, 0, 0, REG_T0),
            ..Default::default()
        };
        let bytes = def.to_bytes();
        let words: [u32; 60] = std::array::from_fn(|i| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()));
        draw.shader.pixel_shader = Some(Arc::new(words));
        draw.shader.combiner_state = Arc::new(words[..57].try_into().unwrap());
        let raw = Arc::make_mut(&mut draw.raw);
        raw.render[..57].copy_from_slice(&words[..57]);
        raw.render[136] = words[54];
        draw.texture_info[0] = Some(d3d::backend::TextureInfo {
            header: 0x1000,
            palette: 0,
            format: gfx::D3DFormat::A8R8G8B8,
            kind: gfx::texture::TextureKind::D2,
            width: 64,
            height: 64,
            depth: 1,
            levels: 1,
        });
        draw.textures[0] = Some(0x1000);
        draw.extras[0].samples_frame = true;
        draw
    }

    #[test]
    fn a_sorted_pool_with_a_lens_and_a_tested_depth_becomes_a_native_pass() {
        let mut draw = pool_draw();
        draw.state.render.depth_mode = gfx::state::DepthBufferMode::Z;
        draw.state.render.depth_write = false;
        draw.state.render.stencil.enable = false;
        let (before, drops) = (counter("layer_plan_15_pool_admitted"), counter("layer_plan_15_pool_on_d3d"));
        let admitted = crate::native_world_layers_glue::with_pool_issue((0, 3), || {
            ResidualDraws::default().admit(&snapshot(&draw))
        });
        let (plan, _) = admitted.expect("a pool with its inputs is admitted");
        assert_eq!(plan.stage, timeline::native::Stage::Water);
        assert!(counter("layer_plan_15_pool_admitted") > before);
        assert_eq!(counter("layer_plan_15_pool_on_d3d"), drops);
    }

    #[test]
    fn each_pool_refusal_is_named_and_leaves_the_guest_d3d_draw() {
        use gfx::state::{CompareFunc, DepthBufferMode};
        let tested = || {
            let mut draw = pool_draw();
            draw.state.render.depth_mode = DepthBufferMode::Z;
            draw.state.render.depth_write = false;
            draw.state.render.stencil.enable = false;
            draw
        };
        let mut cases: Vec<(&str, d3d::ResolvedDraw)> = Vec::new();
        let mut draw = tested();
        draw.target.depth_format = None;
        cases.push(("no_depth", draw));
        let mut draw = tested();
        draw.state.render.depth_func = CompareFunc::Always;
        cases.push(("depth_not_tested", draw));
        let mut draw = tested();
        draw.state.render.depth_write = true;
        cases.push(("writes_depth", draw));
        let mut draw = tested();
        draw.texture_info[0] = None;
        cases.push(("no_lens", draw));
        // A projective bitmap (a ripple) is not a frame sample.
        let mut draw = tested();
        draw.extras[0].samples_frame = false;
        cases.push(("no_lens", draw));
        cases.push(("index_count", { let mut d = tested(); d.indices = Some(vec![0, 1, 2, 0, 1, 2].into()); d }));
        for (cause, draw) in cases {
            let name = format!("layer_plan_15_pool_refused_{cause}");
            let (before, drops) = (counter(&name), counter("layer_plan_15_pool_on_d3d"));
            let admitted = crate::native_world_layers_glue::with_pool_issue((0, 3), || {
                ResidualDraws::default().admit(&snapshot(&draw))
            });
            assert!(admitted.is_none(), "{cause}: the guest's D3D draw stands");
            assert!(counter(&name) > before, "{cause} is counted by name");
            assert!(counter("layer_plan_15_pool_on_d3d") > drops, "{cause} is counted as on D3D");
        }
    }
}
