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
        if let Some((view, count)) = crate::native_world_layers_glue::pool_issue() {
            let stage = timeline::native::Stage::Water;
            if draw.indices.as_deref().is_none_or(|i| i.len() != count as usize) {
                render_bridge::native_layers::count(15, "pool_refused_index_count", 1);
                return None;
            }
            let frame = crate::d3d_diag::presents() + 1;
            let Some(pass) = IndexedPass::for_pool(draw, self.geometry.get(), frame) else {
                render_bridge::native_layers::count(15, "pool_refused_geometry", 1);
                return None;
            };
            return Some((d3d::ExternalPlan { stage, view, ranges: 1, slots: vec![] }, Box::new(pass)));
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
