//! Whole-light admission at the current view target. Every group is from
//! this invocation's item lists. Prepare their ranges in graph order;
//! the device seals all physical copies before the tail can skip a call.
use super::*;
use frame_scene::{LightGroup, LightTarget};
use native_render::light_graph::{self, Node, Pass, Scissor};
use render_bridge::native_lights::TypedPasses;

pub(crate) struct ViewRun<'a> {
    pub(crate) constants: &'a LightConstants,
    pub(crate) scissor: Option<Scissor>,
    pub(crate) groups: Vec<usize>,
}

/// The view's diffuse and specular steps share attachments, projection
/// and clip. A later operation needing another attachment, gel or shadow
/// refuses the entire run, including any earlier diffuse group.
pub(crate) fn view_run<'a>(graph: Pass<'a>, groups: &[&LightGroup]) -> Result<ViewRun<'a>, Keep> {
    let Pass::Light {
        constants, scissor, ..
    } = graph
    else {
        return Err(Keep::Shadow("target"));
    };
    let nodes = light_graph::prepare(graph, groups, |node| match node {
        Node::Constants(_) | Node::Scissor(_) | Node::RestoreView => Ok(None),
        Node::Begin { step, .. }
            if matches!(step.layer, 12 | 13)
                && step.target == LightTarget::View
                && step.stencil == Some(0) =>
        {
            Ok(None)
        }
        Node::End { layer: 12 | 13, .. } => Ok(None),
        Node::Draw { group } => Ok(Some(group)),
        _ => Err(Keep::Unsupported(
            "light requires another target or a shadow operation",
        )),
    })
    .map_err(|reason| match reason {
        light_graph::Refusal::Stage(keep) => keep,
        _ => Keep::Unsupported("incomplete light graph"),
    })?;
    Ok(ViewRun {
        constants,
        scissor,
        groups: nodes.into_iter().flatten().collect(),
    })
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    pub(super) fn typed(
        &self,
        mem: &mut dyn GuestMemory,
        batch: &TypedPasses<'_>,
    ) -> Result<(), Keep> {
        let groups: Vec<_> = batch.groups.iter().map(|g| &g.group).collect();
        let run = view_run(batch.graph, &groups)?;
        let mut device = self
            .device
            .lock()
            .map_err(|_| Keep::Unsupported("device unavailable"))?;
        if !device
            .begin_external_boundary(&mut DeviceMemory(mem))
            .map_err(|_| Keep::Unsupported("could not drain light state"))?
        {
            return Err(Keep::Unsupported("light has an outstanding push callback"));
        }
        if device.bound_target() != batch.view_target {
            return Err(Keep::Unsupported("light view target changed"));
        }
        let view_layers = u8::try_from(run.groups.len())
            .map_err(|_| Keep::Unsupported("invalid light layer count"))?;
        let mut sequence = Vec::new();
        if view_layers != 0 {
            let ready = self.ready(mem)?;
            let mut draws = Vec::new();
            for &i in &run.groups {
                let group = &batch.groups[i];
                let count = group.group.entries.end - group.group.entries.start;
                if usize::try_from(count).ok() != Some(group.entries.len())
                    || group.rows.len() != group.entries.len()
                {
                    return Err(Keep::Unsupported("light group lost entries or pose rows"));
                }
                draws.extend(self.scene_light_draws(&device, mem, &ready, group, run.constants, false)?);
            }
            let [color, depth, x, y, w, h, _, _] = batch.view_target;
            let targets = d3d::ExternalTargets {
                color,
                depth,
                rect: (x, y, w, h),
            };
            // Both layers' target helper reasserts the viewport and clears
            // any inherited scissor. Neither crops to the projected sphere.
            let rect = [x, y, w, h];
            if run
                .scissor
                .is_some_and(|scissor| intersect_scissor(scissor, targets.rect) != rect)
            {
                return Err(Keep::Unsupported(
                    "light scissor differs from the layer viewport",
                ));
            }
            let light = self.prepare_planned(
                &device,
                mem,
                Arc::clone(&ready),
                *run.constants,
                draws,
                None,
                false,
                Some(rect),
            )?;
            let retained = RetainedLight::of(&ready, &light)?;
            let plan = d3d::ExternalPlan {
                stage: timeline::native::Stage::Lights,
                view: batch.view,
                ranges: light.draws.iter().map(|draw| draw.ranges.len()).sum(),
                slots: Vec::new(),
            };
            let pass = RunPass {
                retained: Some(retained),
                light,
                view: None,
                clip: None,
                gel_slot: None,
                fallback: None,
                colour_only: false,
                lighting_only: None,
                cone_shadow: None,
            };
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, &ready)) {
                return Err(Keep::Unsupported("world changed during light preparation"));
            }
            sequence.push(d3d::ExternalStep {
                targets,
                plan,
                pass: Box::new(pass),
            });
        }
        let mut memory = DeviceMemory(mem);
        let held = device
            .prepare_external_sequence(&mut memory, sequence)
            .map_err(|_| Keep::Unsupported("device error preparing the light graph"))?
            .ok_or(Keep::Unsupported(
                "light graph unavailable on a target copy",
            ))?;
        if !held
            .commit_light(
                &mut memory,
                batch.clip,
                &run.constants.frame,
                None,
                view_layers,
                batch.clip_header,
                batch.writes.iter().map(|w| (w.addr, w.bytes.as_slice())),
            )
            .map_err(|_| Keep::Unsupported("device error applying light tail"))?
        {
            return Err(Keep::Unsupported("light tail unavailable"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
