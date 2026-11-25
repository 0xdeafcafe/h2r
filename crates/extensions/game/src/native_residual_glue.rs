//! Admission of sorted pool surfaces, sky strips and residual layer-2 packets.

use native_render::world_layers_gpu::IndexedPass;
use render_bridge::native_sky;

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let enabled = hooks::Switches::from_env().is_ok_and(|s| hooks::switches::native_render_on(&s));
    if enabled && let Ok(mut device) = device.lock() {
        device.set_native_residual(true);
        device.add_draw_admission(std::sync::Arc::new(ResidualDraws::default()));
    }
}

/// The refusal counted (`layer_plan_15_pool_on_d3d`) for each sorted pool
/// draw left on the D3D path.
pub(crate) const POOL_ON_D3D: &str = "pool_on_d3d";

#[derive(Default)]
struct ResidualDraws {
    geometry: crate::native_map::MapIndexedCache,
}

impl d3d::DrawAdmission for ResidualDraws {
    fn admit(
        &self,
        snapshot: &d3d::DrawSnapshot<'_>,
    ) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        use native_render::world_layers::{Owner, Site};

        let draw = snapshot.draw;
        if draw.entry != "D3DDevice_DrawIndexedVertices" {
            return None;
        }
        if crate::native_world_layers_glue::pool_issue().is_some() {
            // The native pool pass drew the bay over the terrain on Delta
            // Towers (cbfe69a). Its depth and lens against the scene are not
            // proven, so the guest's draw stays the D3D packet.
            render_bridge::native_layers::count(15, POOL_ON_D3D, 1);
            return None;
        }
        let call = crate::native_world_layers_glue::indexed_draw()?;
        let (stage, pass) = match (call.layer.site, call.layer.number) {
            (Site::Sky, 15) => {
                let item = crate::native_world_layers_glue::take_sky()?;
                let plan = native_sky::admit(
                    item, call.layer.view, call.index_count, draw.indices.as_ref().map(|i| i.len()),
                    |item| IndexedPass::for_sky(draw, self.geometry.get(), call.frame, call.generation, item),
                );
                match plan {
                    Ok(pass) => {
                        native_sky::count("admitted");
                        (timeline::native::Stage::Sky, pass)
                    }
                    Err(reason) => {
                        native_sky::count(reason);
                        return None;
                    }
                }
            }
            (Site::Scene | Site::AlternateView, 2) if call.layer.list != 0 => {
                if draw.indices.as_ref()?.len() != call.index_count? as usize {
                    return None;
                }
                let stage = match call.geometry?.owner {
                    Owner::Cluster(_) | Owner::Placement(_) => timeline::native::Stage::World,
                    Owner::Object { .. } => timeline::native::Stage::Objects,
                };
                let pass = IndexedPass::new(draw, self.geometry.get(), call.frame, None)?;
                (stage, pass)
            }
            _ => return None,
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

    #[test]
    fn a_sorted_pool_issue_keeps_the_guest_d3d_draw() {
        let draw = indexed_triangle();
        let snapshot = DrawSnapshot {
            draw: &draw,
            vertex_shader: 0,
            pixel_shader: 0,
            bound_textures: [0; 4],
            sampled_stages: [false; 4],
            palettes: [0; 4],
            color_header: 1,
            depth_header: 2,
        };
        let refused = || render_bridge::native_layers::counters().into_iter()
            .find(|(k, _)| *k == format!("layer_plan_15_{POOL_ON_D3D}"))
            .map_or(0, |(_, n)| n);
        let before = refused();
        let admitted = crate::native_world_layers_glue::with_pool_issue((0, 3), || {
            ResidualDraws::default().admit(&snapshot)
        });
        assert!(admitted.is_none(), "a sorted pool must not become a native pass");
        assert!(refused() > before);
    }
}
