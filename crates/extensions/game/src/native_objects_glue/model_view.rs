//! A menu model widget's objects through layer 3's object programs
//! (docs/graphics/native-menus-overlay.md, "Model views"): planned at the
//! overlay's hook as the display path plans an object outside the gather
//! ([`super::display`]: `native_scene_lists::display_model`, then
//! `LitObjects::plan_scene` with the fog its pass takes), and recorded in
//! the overlay's model view pass on its depth
//! ([`native_render::ui_gpu::OVERLAY_DEPTH`]) with the view's own
//! constants ([`native_render::ui_gpu::ModelViewObjects`]).
use super::*;
use render_bridge::native_scene_lists::{self, LodDetail};

/// The draws of a model view's objects, by handle, planned at the hook.
pub(crate) struct Planned {
    /// The map's resident models; `None` while they load, when the
    /// views draw no object yet.
    models: Option<Arc<Models>>,
    objects: HashMap<u32, Vec<(u32, PlannedDraw)>>,
}

/// Plans each of `handles` (the overlay's model views' objects) as the
/// display path plans an object: its records at the highest detail (a
/// widget's object fills its box), each section's parts through the
/// mounted model's programs, with the fog their passes take. What the
/// native programs cannot draw stops by name; nothing falls back to the
/// game's draw.
pub(crate) fn plan(mem: &dyn GuestMemory, handles: &[u32]) -> Planned {
    let mut planned = Planned { models: super::layer_source_models(mem), objects: HashMap::new() };
    let Some(models) = planned.models.clone() else { return planned };
    let entry = LayerEntry::read(mem);
    let fog = device_state::fog_words(mem);
    let specular = device_specular(mem);
    let mut lit = models.lit.lock().unwrap_or_else(|e| e.into_inner());
    let Some(lit) = lit.as_mut() else {
        planned.models = None;
        return planned;
    };
    let covers = models.coverage.read().unwrap_or_else(|e| e.into_inner());
    for &handle in handles {
        if planned.objects.contains_key(&handle) {
            continue;
        }
        let Some((frame, model, sections)) = native_scene_lists::display_model(mem, handle, LodDetail::Highest, None) else {
            unimplemented!("ui model: object {handle:#x} has no display records");
        };
        if models.index.get(&(frame.definition as u16)) != Some(&model) {
            unimplemented!("ui model: object {handle:#x}'s render model {model:#x} is not the mounted map's");
        }
        let Some(cover) = covers.get(&TagId(model)) else {
            unimplemented!("ui model: object {handle:#x}'s render model {model:#x} has no native programs");
        };
        let mut draws = Vec::new();
        for section in &sections {
            let Some(parts) = cover.display_parts.get(&section.section) else {
                unimplemented!("ui model: object {handle:#x} section {} has no native parts", section.section);
            };
            for &(part, shader) in parts {
                let key = frame_scene::ItemKey {
                    record: 0,
                    shader,
                    section: section.section,
                    part,
                    implementation: section.implementation,
                };
                let mut draw = lit
                    .plan_scene(model, &frame, &key, &section.rows, &entry.states)
                    .unwrap_or_else(|why| unimplemented!("ui model: object {handle:#x} part {part}: {why}"));
                if layer_draw_fog(lit, model, &mut draw, &fog, specular).is_none() {
                    unimplemented!("ui model: object {handle:#x} part {part}: its fog does not resolve");
                }
                if draw.texaccum {
                    unimplemented!("ui model: object {handle:#x} part {part} samples the accumulation target");
                }
                draws.push((model, draw));
            }
        }
        planned.objects.insert(handle, draws);
    }
    drop(covers);
    planned
}

/// [`Planned`]'s draws recorded on the overlay's target `t`.
pub(crate) struct Objects<'a> {
    pub planned: &'a Planned,
    pub t: &'a gfx::external::ExternalAttachments,
}

impl native_render::ui_gpu::ModelViewObjects for Objects<'_> {
    fn draws(
        &mut self,
        view: &native_render::model_view::PlanModelView,
        inputs: &ViewInputs,
        color: wgpu::TextureFormat,
        size: (u32, u32),
    ) -> Result<Vec<gfx::RecordedDraw>, &'static str> {
        let Some(models) = &self.planned.models else { return Ok(Vec::new()) };
        let t = self.t;
        let depth = native_render::ui_gpu::OVERLAY_DEPTH;
        let mut lit = models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let lit = lit.as_mut().ok_or("planned object geometry unavailable")?;
        // The widget's order; an object whose pipelines are still compiling
        // waits a frame, as the display path leaves it out, and the rest
        // draw.
        let mut draws = Vec::new();
        for handle in &view.objects {
            let Some(planned) = self.planned.objects.get(handle) else { continue };
            if planned.iter().any(|(model, draw)| !lit.has_draw_assets(*model, draw)) {
                return Err("planned object geometry or material unavailable");
            }
            if lit.prewarm_draws(&t.device, color, depth, planned) {
                draws.extend_from_slice(planned);
            }
        }
        if draws.is_empty() {
            return Ok(Vec::new());
        }
        // The view's lens is the widget's own projection (its inputs), not
        // the display's: no display factor on it.
        let recorded = lit.draws(&t.device, &t.queue, &t.uploads, color, depth, size, inputs, 1.0, &draws, None)?;
        // The persistent rows they read, written before the submission.
        for (buffer, offset, bytes) in &recorded.writes {
            t.queue.write_buffer(buffer, *offset, bytes);
        }
        Ok(recorded.packets)
    }
}
