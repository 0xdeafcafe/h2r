//! Draw an object outside the authoritative gather when the mounted model
//! determines the whole opaque layer without a guest render record.
use super::*;
use frame_scene::{ItemKey, NodeMatrix, Row};

pub(super) struct RigidParts {
    inverse_binds: Vec<NodeMatrix>,
    keys: Vec<(ItemKey, usize)>,
}

impl RigidParts {
    pub(super) fn from_model(
        geo: &RenderModelGeometry, lit: &LitModelInput, required: &HashMap<u16, u8>,
    ) -> Option<Self> {
        // Each region must have one possible geometry across all of its
        // permutations and detail levels. A hidden permutation declines.
        let mut selected = HashSet::new();
        for region in &geo.regions {
            let section = region.permutations.first()?.best_section()?;
            if region.permutations.iter().any(|p| p.best_section() != Some(section)
                || p.lod_sections.iter().flatten().any(|&s| s != section))
            {
                return None;
            }
            selected.insert(section);
        }
        let inverse_binds = geo.nodes.iter().map(|n|
            native_render::model_load::inverse_bind(&n.wire.unknown_028[..])
        ).collect::<Option<Vec<_>>>()?;
        let mut keys = Vec::new();
        for p in &lit.parts {
            let index = usize::from(p.section);
            if !selected.contains(&index) { continue; }
            let section = geo.sections.get(index)?;
            // A rigid section uses one matrix. The local node map (empty
            // for root) must identify it without the per-part palette.
            if section.wire.geometry_classification.0 != 1 { return None; }
            let node = usize::from(section.node_map.first().copied().unwrap_or(0));
            if inverse_binds.get(node).is_none()
                || section.node_map.iter().any(|&n| usize::from(n) != node)
                || (inverse_binds.len() != 1 && section.node_map.is_empty())
            {
                return None;
            }
            let raw = section.geometry.parts.get(usize::from(p.part))?;
            let mask = *required.get(&raw.material)?;
            if mask.count_ones() != 1 { return None; }
            keys.push((ItemKey {
                record: 0, shader: p.shader_tag, section: p.section, part: p.part,
                implementation: mask.trailing_zeros() as usize,
            }, node));
        }
        (!keys.is_empty()).then_some(Self { inverse_binds, keys })
    }
}

/// A single-node section has one palette entry for every vertex. Transform
/// model-space positions through the inverse bind and the current pose.
fn rigid_rows(pose: NodeMatrix, bind: NodeMatrix) -> [Row; 3] {
    let m = pose.compose(bind);
    std::array::from_fn(|i| [
        m.scale * m.forward[i], m.scale * m.left[i], m.scale * m.up[i], m.position[i],
    ])
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    pub(super) fn draw_display_models(&self, mem: &mut dyn GuestMemory, view: &View) -> Option<()> {
        let handles = &view.display_visibility.as_ref()?.objects;
        let (models, _) = self.load(mem)?;
        let mut device = self.device.lock().ok()?;
        if !device.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem)).ok()?
            || !model_palettes_hold(&models, &device, mem)
            || !crate::native_layers_glue::viewport_clip(&device, &crate::native_world_glue::ReadMemory(mem))
        {
            return None;
        }
        let entry = LayerEntry::read(&device, mem);
        if !entry.w_buffered { return None; }
        let fog = crate::native_fog_glue::device_fog_words(&device, mem);
        let specular = device_specular(&device, mem);
        let read = crate::native_world_glue::ReadMemory(mem);
        let (gpu, color, depth) = device.external_pipeline_target(&read)?;
        let mut draws = Vec::new();
        {
            let mut gpu_models = models.lit.try_lock().ok()?;
            let lit = gpu_models.as_mut()?;
            let covers = models.coverage.read().ok()?;
            for &handle in handles {
                let Some(frame) = native_render::objects::read_object(&read, handle) else { continue };
                let Some(&model) = models.index.get(&(frame.definition as u16)) else { continue };
                let Some(parts) = covers.get(&TagId(model)).and_then(|c| c.display.as_ref()) else { continue };
                if parts.inverse_binds.len() != frame.nodes.len() { continue; }
                let start = draws.len();
                for (key, node) in &parts.keys {
                    if frame.nodes.len() > 1 && native_render::object_nodes::section_node(
                        &read, model, u32::from(key.section),
                    ).ok().and_then(|n| usize::try_from(n).ok()) != Some(*node) {
                        break;
                    }
                    let rows = rigid_rows(frame.nodes[*node], parts.inverse_binds[*node]);
                    let Ok(mut draw) = lit.plan_scene(model, &frame, key, &rows, &entry.states) else { break };
                    if layer_draw_fog(lit, model, &mut draw, &fog, specular).is_none() { break; }
                    draws.push((model, draw));
                }
                if draws.len() - start != parts.keys.len() {
                    draws.truncate(start);
                }
            }
            if !lit.prewarm_draws(&gpu, color, depth, &draws) { return None; }
        }
        let texaccum = if draws.iter().any(|(_, d)| d.texaccum) {
            if !device.samples_levels_in_place() { return None; }
            Some(native_render::world_layers::Texaccum::read(&Ro(mem))?)
        } else {
            None
        };
        let plan = d3d::ExternalPlan {
            stage: timeline::native::Stage::Objects, view: u32::from(view.key.view),
            ranges: draws.len(), slots: Vec::new(),
        };
        let pass = ObjectsPass {
            display_only: true, models, draws, texaccum, view: None, w_clip: None,
            run: u64::from(device.swap_count()).wrapping_add(1),
        };
        device.draw_display_stage(&mut crate::native_lights_glue::DeviceMemory(mem), plan, Box::new(pass))
            .ok()?.then_some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rigid_palette_places_vertices_in_the_current_pose() {
        let pose = NodeMatrix {
            scale: 2., forward: [0., 1., 0.], left: [-1., 0., 0.],
            position: [2., 3., 5.], ..NodeMatrix::IDENTITY
        };
        let bind = NodeMatrix { position: [1., 2., 3.], ..NodeMatrix::IDENTITY };
        let rows = rigid_rows(pose, bind);
        let point = [2., 5., -1.];
        let output = rows.map(|r| r[0]*point[0] + r[1]*point[1] + r[2]*point[2] + r[3]);
        assert_eq!(output, pose.point(bind.point(point)));
    }
}
