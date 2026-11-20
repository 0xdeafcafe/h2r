//! Admission of the typed stage already reserved by the device. Its
//! shared renderer and complete attachment list identify its place in
//! the worker's command stream.

use std::collections::HashMap;
use std::sync::Arc;

use frame_scene::TargetRef;

use super::{Worker, bind};
use crate::backend::{Backend, SceneStage};

/// The normal worker path, also used when the frame cannot be adopted.
pub(super) fn issue<B: Backend>(stage: SceneStage<u32>, w: &mut Worker<B>) -> Result<(), String> {
    let SceneStage { surfaces, request, plan, pass } = stage;
    let kind = plan.stage;
    let ranges = plan.ranges;
    let Some(surfaces) = surfaces.try_map(|s| w.surfaces.get(s).copied()) else {
        crate::scene_stage::dropped(kind, ranges, "a target is not live on the render thread");
        return Ok(());
    };
    if !w.inner.runs_scene_stages() {
        crate::scene_stage::dropped(kind, ranges, "the backend stopped taking scene stages");
        return Ok(());
    }
    if !w.inner.run_scene_stage(SceneStage { surfaces, request, plan, pass })? {
        crate::scene_stage::dropped(kind, ranges, "the render thread could not record it");
    }
    Ok(())
}

pub(super) fn bind_targets(
    targets: &mut HashMap<TargetRef, u32>,
    kept: &SceneStage<TargetRef>,
    queued: &SceneStage<u32>,
) -> Option<()> {
    if !Arc::ptr_eq(&kept.pass, &queued.pass)
        || !kept.pass.lock().ok()?.replays_scene()
        || kept.request != queued.request
        || kept.plan.stage != queued.plan.stage
        || kept.plan.view != queued.plan.view
        || kept.plan.ranges != queued.plan.ranges
    {
        return None;
    }

    // Include writes and the guest's slot choice, even if two plans
    // happen to bind the same set of surfaces.
    let slot = |p: &Option<crate::ExternalSlot>| p.map(|s| (s.header, s.written));
    if !kept.plan.slots.iter().map(slot).eq(queued.plan.slots.iter().map(slot)) {
        return None;
    }
    let k = &kept.surfaces;
    let q = &queued.surfaces;
    if k.sampled.len() != q.sampled.len() || k.slots.len() != q.slots.len() || k.slot_sizes != q.slot_sizes {
        return None;
    }
    bind(targets, k.color, Some(q.color))?;
    bind(targets, k.depth, Some(q.depth))?;
    for (&target, &id) in k.sampled.iter().zip(&q.sampled) {
        bind(targets, target, Some(id))?;
    }
    for (target, id) in k.slots.iter().zip(&q.slots) {
        bind(targets, target.unwrap_or(TargetRef::None), *id)?;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{ExternalPass, ExternalPassRequest, ExternalPlan, ExternalSlot, ExternalSurfaces, SharedReplacement};
    use std::sync::Mutex;
    use timeline::native::Stage;

    struct Pass(bool);
    impl ExternalPass for Pass {
        fn replays_scene(&self) -> bool { self.0 }
    }

    fn pair(repeatable: bool) -> (SceneStage<TargetRef>, SceneStage<u32>) {
        let sampled = TargetRef::Aux(0);
        let surfaces = ExternalSurfaces {
            color: TargetRef::SceneColour,
            depth: TargetRef::SceneDepth,
            sampled: vec![sampled],
            slots: vec![None, Some(sampled)],
            slot_sizes: vec![None, Some((8, 8))],
        };
        let pass: SharedReplacement = Arc::new(Mutex::new(Box::new(Pass(repeatable))));
        let plan = ExternalPlan {
            stage: Stage::Fog, view: 1, ranges: 2,
            slots: vec![None, Some(ExternalSlot { header: 51, written: true })],
        };
        let request = ExternalPassRequest { rect: (0, 0, 8, 8), surface_size: (8, 8), z_scale: 256.0 };
        let physical = surfaces.try_map(|s| match s {
            TargetRef::SceneColour => Some(5),
            TargetRef::SceneDepth => Some(9),
            TargetRef::Aux(0) => Some(13),
            _ => None,
        }).unwrap();
        (
            SceneStage { surfaces, plan: plan.clone(), request, pass: Arc::clone(&pass) },
            SceneStage { surfaces: physical, plan, request, pass },
        )
    }

    #[test]
    fn admission_matches_the_reserved_pass_and_all_its_attachment_slots() {
        let (kept, mut queued) = pair(true);
        let mut targets = HashMap::new();
        assert_eq!(bind_targets(&mut targets, &kept, &queued), Some(()));
        assert_eq!(targets, HashMap::from([
            (TargetRef::SceneColour, 5), (TargetRef::SceneDepth, 9), (TargetRef::Aux(0), 13),
        ]));

        queued.surfaces.slots[1] = Some(19);
        assert!(bind_targets(&mut HashMap::new(), &kept, &queued).is_none(), "a sampled slot changed");
        queued.surfaces.slots[1] = Some(13);
        queued.plan.slots[1].as_mut().unwrap().written = false;
        assert!(bind_targets(&mut HashMap::new(), &kept, &queued).is_none(), "different affected surfaces");
        queued.plan.slots[1].as_mut().unwrap().written = true;
        queued.pass = Arc::new(Mutex::new(Box::new(Pass(true))));
        assert!(bind_targets(&mut HashMap::new(), &kept, &queued).is_none(), "a different pass with the same plan");

        let (kept, queued) = pair(false);
        assert!(bind_targets(&mut HashMap::new(), &kept, &queued).is_none(), "a one-shot pass");
    }
}
