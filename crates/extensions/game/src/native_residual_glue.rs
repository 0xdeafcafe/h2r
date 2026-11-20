//! The native renderer's remaining Direct3D draws: the sky model's
//! indexed strip and the layer-2 world and object sections, admitted
//! with the same resolved input and two-copy reservation as the opaque
//! layers.

/// Use the same resolved indexed input and two-copy reservation as the
/// opaque layers. No sky model state, sort order or layer handler is moved.
pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let enabled = hooks::Switches::from_env().is_ok_and(|s| hooks::switches::native_render_on(&s));
    if enabled && let Ok(mut device) = device.lock() {
        device.set_native_residual(true);
        device.add_draw_admission(std::sync::Arc::new(ResidualDraws::default()));
    }
}

#[derive(Default)]
struct ResidualDraws {
    geometry: std::sync::Arc<native_render::world_layers_gpu::Cache>,
}

impl d3d::DrawAdmission for ResidualDraws {
    fn admit(
        &self,
        snapshot: &d3d::DrawSnapshot<'_>,
    ) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        use native_render::world_layers::{Owner, Site};

        let call = crate::native_world_layers_glue::indexed_draw()?;
        let draw = snapshot.draw;
        if draw.entry != "D3DDevice_DrawIndexedVertices"
            || draw.indices.as_ref()?.len() != call.index_count? as usize
        {
            return None;
        }
        let stage = match (call.layer.site, call.layer.number) {
            (Site::Sky, 15) => timeline::native::Stage::World,
            (Site::Scene | Site::AlternateView, 2) if call.layer.list != 0 => {
                match call.geometry?.owner {
                    Owner::Cluster(_) | Owner::Placement(_) => timeline::native::Stage::World,
                    Owner::Object { .. } => timeline::native::Stage::Objects,
                }
            }
            _ => return None,
        };
        let pass = native_render::world_layers_gpu::IndexedPass::new(
            draw,
            std::sync::Arc::clone(&self.geometry),
            call.frame,
            None,
        )?;
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
