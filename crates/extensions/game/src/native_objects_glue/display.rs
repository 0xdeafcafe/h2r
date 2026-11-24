//! Draw an object outside the authoritative gather when the mounted model
//! determines the whole opaque layer without a guest render record.
use super::*;
use frame_scene::ItemKey;
use render_bridge::native_scene_lists;

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
        let target = device.external_ready(&read, false).ok()?;
        if device.projection_factor_for_target(target.surface_size) >= 1.0 { return None; }
        let (gpu, color, depth) = device.external_pipeline_target(&read)?;
        let mut draws = Vec::new();
        {
            let mut gpu_models = models.lit.try_lock().ok()?;
            let lit = gpu_models.as_mut()?;
            let covers = models.coverage.read().ok()?;
            for &handle in handles {
                if crate::forge_controls::parked() == Some(handle) { continue; }
                let Some((frame, model, sections)) = native_scene_lists::display_model(mem, handle) else { continue };
                if models.index.get(&(frame.definition as u16)) != Some(&model) { continue; }
                let Some(cover) = covers.get(&TagId(model)) else { continue };
                let start = draws.len();
                let complete = sections.iter().all(|section| {
                    let Some(parts) = cover.display_parts.get(&section.section) else { return false };
                    parts.iter().all(|&(part, shader)| {
                        let key = ItemKey { record: 0, shader, section: section.section, part,
                            implementation: section.implementation };
                        let Ok(mut draw) = lit.plan_scene(model, &frame, &key, &section.rows, &entry.states) else { return false };
                        if layer_draw_fog(lit, model, &mut draw, &fog, specular).is_none() { return false; }
                        draws.push((model, draw));
                        true
                    })
                });
                if !complete { draws.truncate(start); }
            }
            if draws.is_empty() || !lit.prewarm_draws(&gpu, color, depth, &draws) { return None; }
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
