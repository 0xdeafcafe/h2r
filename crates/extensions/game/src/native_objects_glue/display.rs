//! Draw an object outside the authoritative gather when the mounted model
//! determines the whole opaque layer without a guest render record.
use super::*;
use frame_scene::ItemKey;
use render_bridge::native_scene_lists::{self, DisplayGeometry, LodDetail};

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    pub(super) fn draw_display_models(&self, mem: &mut dyn GuestMemory, view: &View, lod: LodDetail) -> Option<()> {
        let visibility = view.display_visibility.as_ref()?;
        let handles = &visibility.objects;
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
        // A widened guest camera's objects outside the retail set are drawn
        // whatever the device's own factor (`scene_step::retail_object`).
        if !visibility.guest_lens && device.projection_factor_for_target(target.surface_size) >= 1.0 { return None; }
        let (gpu, color, depth) = device.external_pipeline_target(&read)?;
        let mut draws = Vec::new();
        {
            let mut gpu_models = models.lit.try_lock().ok()?;
            let lit = gpu_models.as_mut()?;
            let covers = models.coverage.read().ok()?;
            for &handle in handles {
                if crate::forge_controls::parked() == Some(handle) { continue; }
                let Some((frame, model, sections)) = native_scene_lists::display_model(mem, handle, lod, None) else { continue };
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
            display_only: true, display_draws: None, models, draws, texaccum, view: None, w_clip: None,
            run: u64::from(device.swap_count()).wrapping_add(1),
        };
        device.draw_display_stage(&mut crate::native_lights_glue::DeviceMemory(mem), plan, Box::new(pass))
            .ok()?.then_some(())
    }
}

/// Plan detail substitutions only for complete objects within one admitted
/// run. Every original packet remains available for the reference and for
/// display fallback when the private geometry cannot be drawn.
pub(super) fn plan_lod<B: d3d::Backend>(
    models: &Models,
    device: &d3d::Device<B>,
    mem: &dyn GuestMemory,
    scene: &FrameScene,
    view: &View,
    run: &[Entry],
    reference: &[(u32, PlannedDraw)],
    lod: LodDetail,
) -> Option<Vec<(u32, PlannedDraw)>> {
    let (entry, w_buffered) = entry_states(device, mem);
    let stencil = mem.u32(native_render::skip::STENCIL_OFF_FLAGS).ok()?;
    if !w_buffered || stencil & 0x00ff_ff00 != 0 {
        return None;
    }
    let fog = crate::native_fog_glue::device_fog_words(device, mem);
    let specular = device_specular(device, mem);
    let read = crate::native_world_glue::ReadMemory(mem);
    let (gpu, color, depth) = device.external_pipeline_target(&read)?;
    let mut lit = models.lit.try_lock().ok()?;
    let lit = lit.as_mut()?;
    let coverage = models.coverage.read().ok()?;
    let parts = run.iter().filter_map(|item| match item {
        Entry::Object { handle, part: Some(part) } => Some((*handle, part.key)),
        _ => None,
    }).collect::<Vec<_>>();
    if parts.len() != reference.len() {
        return None;
    }
    let mut changed = false;
    let mut display = Vec::with_capacity(reference.len());
    let mut start = 0;
    for group in parts.chunk_by(|a, b| a.0 == b.0) {
        let end = start + group.len();
        let handle = group[0].0;
        let replacement = (|| {
            let instance = scene.instance(view, InstanceKey(handle))?;
            let original = scene.parts_of(instance);
            if original.len() != group.len() || original.iter().zip(group).any(|(part, (_, key))|
                part.layer != 3 || part.key != *key || part.subparts.is_some()
                    || part.flags.contains(PartFlags::AUX_DEPTH)
                    || part.flags.contains(PartFlags::STENCIL_MARK)
            ) {
                return None;
            }
            // A different section set can replace the entire object only if
            // the retail run submitted all of its opaque parts.
            let model = reference[start].0;
            let cover = coverage.get(&TagId(model))?;
            let mounted = DisplayGeometry { model, node_maps: &cover.node_maps };
            let (frame, model, sections) = native_scene_lists::display_model(mem, handle, lod, Some(mounted))?;
            if sections.is_empty() {
                return None;
            }
            let mut replacement = Vec::new();
            for section in sections {
                for &(part, shader) in cover.display_parts.get(&section.section)? {
                    let key = ItemKey {
                        record: 0, shader, section: section.section, part,
                        implementation: section.implementation,
                    };
                    let mut draw = lit.plan_scene(model, &frame, &key, &section.rows, &entry.states).ok()?;
                    if draw.texaccum != reference[start].1.texaccum {
                        return None;
                    }
                    layer_draw_fog(lit, model, &mut draw, &fog, specular)?;
                    replacement.push((model, draw));
                }
            }
            if replacement.is_empty() {
                return None;
            }
            Some(replacement)
        })();
        if let Some(replacement) = replacement {
            changed = true;
            display.extend(replacement);
        } else {
            display.extend_from_slice(&reference[start..end]);
        }
        start = end;
    }
    (changed && lit.prewarm_draws(&gpu, color, depth, &display)).then_some(display)
}
