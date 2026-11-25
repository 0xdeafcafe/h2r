//! E3's shadowed cones in the frame graph (docs/graphics/e3-lighting.md,
//! "Built: the shadowed cones"). The frame step draws a chosen light the
//! renderer shadows with its shadowed implementations, whose layers read
//! layer 11's term from the destination alpha, and its scene stage names
//! the light (`d3d::ExternalPass::cone_shadow`). Under E3, just before
//! each such stage a cone stage writes that term: the frame's first also
//! draws every named cone's tile into the local lights' atlas
//! (`cone_shadows_gpu::ConeShadows`), and each draws its own receivers'
//! term into the scene's alpha, at the guest's W-buffer depth, as the
//! sun's receivers do ([`super::sun`]).
//!
//! A named light whose cone the frame cannot draw (no cone of the
//! render thread's choice by its handle, a depth the receivers cannot
//! meet, an empty viewport, or nothing under the cone to receive) is kept:
//! its stage is not applied, since its passes would read an alpha no one
//! wrote for them. Retail never plans any of this.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use d3d::Lighting;
use d3d::backend::{
    ExternalAttachments, ExternalPass, ExternalPassRequest, ExternalPlan, ExternalSurfaces,
    SceneStage,
};
use d3d::scene_sink::{ReplayFrame, ReplayOp};
use frame_scene::{D24_SCALE, FrameScene, NodeMatrix, TargetRef};
use gfx::D3DFormat;
use gfx::external::ExternalEncoded;

use super::sun::{RECEIVER_DEPTH_BIAS, clip_rows, world_mesh};
use crate::cone_shadows::{self, Cone, ShadowedCone, Source};
use crate::cone_shadows_gpu::{ConeShadows, ConeTile};
use crate::sun_casters::{self, CasterModels, ReceiverKey, ReceiverMesh, WorldReceivers};
use crate::sun_tiles_gpu::{SunFrame, SunMesh, SunReceiver};

/// What the cones drew in one draw of the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConeDraws {
    /// The renderer's cone tiles drawn.
    pub cones_drawn: u32,
    /// Shadowed chosen lights whose cone could not be drawn: their stages
    /// were not applied.
    pub cones_kept: u32,
    pub tile_draws: u32,
    pub receiver_draws: u32,
}

/// What one draw of the frame does about the cones.
pub(crate) struct ConeReplay {
    /// A cone stage to apply before the op at each index: the shadowed
    /// chosen light's stage.
    pub before: HashMap<usize, ReplayOp>,
    /// Shadowed chosen lights' stages not applied ([`ConeDraws::cones_kept`]).
    pub skip: HashSet<usize>,
    state: Arc<Mutex<FrameCones>>,
}

impl ConeReplay {
    /// The cones' work this draw, once its stages have recorded.
    pub fn work(&self) -> ConeDraws {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).work
    }
}

/// One cone the frame draws, with what casts into its tile and what
/// receives it.
struct Planned {
    cone: Cone,
    slot: usize,
    flashlight: bool,
    casters: Vec<ReceiverMesh>,
    receivers: Vec<ReceiverMesh>,
}

/// The frame's cones, shared by their stages in op order.
struct FrameCones {
    cones: Vec<Planned>,
    /// Each cone's rectangle in the atlas, once the tiles are drawn.
    rects: Option<Vec<[u32; 4]>>,
    work: ConeDraws,
}

/// Each device's atlas, kept across frames.
static ATLASES: Mutex<Vec<(wgpu::Device, Arc<Mutex<ConeShadows>>)>> = Mutex::new(Vec::new());

fn atlas_for(device: &wgpu::Device) -> Arc<Mutex<ConeShadows>> {
    let mut all = ATLASES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, a)) = all.iter().find(|(d, _)| d == device) {
        return Arc::clone(a);
    }
    let made = Arc::new(Mutex::new(ConeShadows::new(device)));
    all.push((device.clone(), Arc::clone(&made)));
    made
}

/// The shadowed chosen light a stage op names, by handle.
fn named(op: &ReplayOp) -> Option<(u32, &SceneStage<TargetRef>)> {
    let ReplayOp::Stage(stage) = op else {
        return None;
    };
    let handle = stage.pass.lock().ok()?.cone_shadow()?;
    Some((handle, stage))
}

/// The renderer's cone of `handle` in the view `at_view` (a
/// `ViewKey::view`), else in any view.
fn cone_of<'a>(
    scene: &FrameScene,
    renderer: &'a [ShadowedCone],
    handle: u32,
    at_view: u8,
) -> Option<&'a ShadowedCone> {
    let mut of = renderer.iter().filter(|c| c.handle == handle);
    let in_view = of.clone().find(|c| {
        scene
            .views
            .get(c.view as usize)
            .is_some_and(|v| v.key.view == at_view)
    });
    in_view.or_else(|| of.next())
}

/// An instance of `cone`'s view, skinned for this scene; `None` when it
/// has no finite nodes or no loaded model.
fn instance_mesh(
    scene: &FrameScene,
    cone: &ShadowedCone,
    key: frame_scene::InstanceKey,
    models: &dyn CasterModels,
) -> Option<ReceiverMesh> {
    let view = scene.views.get(cone.view as usize)?;
    let instance = scene.instances_of(view).iter().find(|i| i.key == key)?;
    let nodes = scene.nodes_of(instance);
    if nodes.is_empty() || !nodes.iter().all(NodeMatrix::is_finite) {
        return None;
    }
    let mesh = sun_casters::skin(models.model(instance.definition)?, nodes);
    if mesh.indices.is_empty() {
        return None;
    }
    let (centre, radius) = instance.sphere;
    Some(ReceiverMesh {
        key: ReceiverKey::Object(key),
        mesh: Arc::new(mesh),
        centre,
        radius,
    })
}

/// `cone`'s casters and receivers: the view's instances the gather picks
/// (`cone_shadows::casters`, `receivers`), skinned, and the structure
/// parts of its clusters under the cone, which do both.
fn gather(
    scene: &FrameScene,
    cone: &ShadowedCone,
    models: Option<&dyn CasterModels>,
    world: Option<&dyn WorldReceivers>,
) -> (Vec<ReceiverMesh>, Vec<ReceiverMesh>) {
    let structure = match (
        world.filter(|w| w.current()),
        scene.views.get(cone.view as usize),
    ) {
        (Some(w), Some(v)) => w.cone_parts(scene.clusters_of(v), &cone.cone),
        _ => Vec::new(),
    };
    let skinned = |keys: Vec<frame_scene::InstanceKey>| -> Vec<ReceiverMesh> {
        let Some(models) = models else {
            return Vec::new();
        };
        keys.into_iter()
            .filter_map(|k| instance_mesh(scene, cone, k, models))
            .collect()
    };
    let mut casters = skinned(cone_shadows::casters(scene, cone));
    let mut receivers = skinned(cone_shadows::receivers(scene, cone));
    casters.extend(structure.iter().cloned());
    receivers.extend(structure);
    (casters, receivers)
}

/// Plans the cones for one draw of `frame` from `scene`: `None` in any
/// mode but E3 and when no stage names a cone.
pub(crate) fn plan(
    scene: &FrameScene,
    frame: &ReplayFrame,
    lighting: Lighting,
) -> Option<ConeReplay> {
    if lighting != Lighting::E3 {
        return None;
    }
    let named_ops: Vec<(usize, u8, u32, &SceneStage<TargetRef>)> = frame
        .ops
        .iter()
        .enumerate()
        .filter_map(|(i, (at, op))| named(op).map(|(h, s)| (i, at.view, h, s)))
        .collect();
    if named_ops.is_empty() {
        return None;
    }
    let lit = crate::light_choice::choose(scene);
    let choice = cone_shadows::choose(scene, &lit);
    let renderer: Vec<ShadowedCone> = choice
        .cones
        .iter()
        .filter(|c| c.source == Source::Renderer && c.slot.is_some())
        .copied()
        .collect();
    let models = sun_casters::models_for(scene.id.map_epoch);
    let models = models.as_deref().map(|m| m as &dyn CasterModels);
    let world = sun_casters::world_for(scene.id.map_epoch);
    let world = world.as_deref().map(|w| w as &dyn WorldReceivers);
    let depth_ok = frame.depth.is_some_and(|d| {
        d.format == D3DFormat::D24S8 as u32 || d.format == D3DFormat::LinD24S8 as u32
    });
    let mut cones: Vec<Planned> = Vec::new();
    // Each planned cone's place, by its slot.
    let mut by_slot: HashMap<usize, usize> = HashMap::new();
    let mut before = HashMap::new();
    let mut skip = HashSet::new();
    let mut work = ConeDraws::default();
    // The stages, made once the cones are all planned.
    let mut stages: Vec<(usize, usize, &SceneStage<TargetRef>, u32)> = Vec::new();
    for (i, at_view, handle, stage) in named_ops {
        let planned: Option<(usize, u32)> = 'plan: {
            if !depth_ok {
                break 'plan None;
            }
            let Some(cone) = cone_of(scene, &renderer, handle, at_view) else {
                break 'plan None;
            };
            let Some(slot) = cone.slot else {
                break 'plan None;
            };
            if let Some(&k) = by_slot.get(&slot) {
                break 'plan Some((k, cone.view));
            }
            let (casters, receivers) = gather(scene, cone, models, world);
            if receivers.is_empty() {
                break 'plan None;
            }
            cones.push(Planned {
                cone: cone.cone,
                slot,
                flashlight: cone.flashlight,
                casters,
                receivers,
            });
            by_slot.insert(slot, cones.len() - 1);
            Some((cones.len() - 1, cone.view))
        };
        match planned {
            Some((k, view)) => stages.push((i, k, stage, view)),
            None => {
                skip.insert(i);
                work.cones_kept += 1;
            }
        }
    }
    let state = Arc::new(Mutex::new(FrameCones {
        cones,
        rects: None,
        work,
    }));
    for (i, k, stage, view) in stages {
        match cone_stage(scene, frame, view, k, stage, &state) {
            Some(op) => {
                before.insert(i, op);
            }
            None => {
                skip.insert(i);
                state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .work
                    .cones_kept += 1;
            }
        }
    }
    Some(ConeReplay {
        before,
        skip,
        state,
    })
}

/// The cone stage before a shadowed chosen light's `stage`: cone `k` of
/// the frame's, on scene view `view`, on the light's own attachments and
/// rectangle.
fn cone_stage(
    scene: &FrameScene,
    frame: &ReplayFrame,
    view: u32,
    k: usize,
    stage: &SceneStage<TargetRef>,
    state: &Arc<Mutex<FrameCones>>,
) -> Option<ReplayOp> {
    let camera = &scene.views.get(view as usize)?.camera;
    frame.colour?;
    let (_, _, w, h) = stage.request.rect;
    if w == 0 || h == 0 || camera.far <= 0.0 {
        return None;
    }
    let pass = ConeStage {
        state: Arc::clone(state),
        cone: k,
        frame: SunFrame {
            clip: clip_rows(&camera.view_projection()),
            depth: [D24_SCALE, 1.0 / camera.far, 1.0, RECEIVER_DEPTH_BIAS],
            viewport: None,
        },
    };
    Some(ReplayOp::Stage(SceneStage {
        surfaces: ExternalSurfaces {
            color: stage.surfaces.color,
            depth: stage.surfaces.depth,
            sampled: vec![],
            slots: vec![],
            slot_sizes: vec![],
        },
        request: stage.request,
        plan: ExternalPlan {
            stage: timeline::native::Stage::Shadows,
            view: stage.plan.view,
            ranges: 1,
            slots: vec![],
        },
        pass: Arc::new(Mutex::new(Box::new(pass) as Box<dyn ExternalPass>)),
    }))
}

/// A world part's upload is kept per device; an object's is the frame's.
fn upload(device: &wgpu::Device, r: &ReceiverMesh) -> Option<SunMesh> {
    match r.key {
        ReceiverKey::World(..) => world_mesh(device, &r.mesh).ok(),
        ReceiverKey::Object(_) => {
            SunMesh::new(device, &r.mesh.positions, &r.mesh.normals, &r.mesh.indices).ok()
        }
    }
}

/// One shadowed light's term; the frame's first also draws the tiles.
struct ConeStage {
    state: Arc<Mutex<FrameCones>>,
    cone: usize,
    frame: SunFrame,
}

impl ExternalPass for ConeStage {
    fn replays_scene(&self) -> bool {
        true
    }

    fn record_encoded(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalEncoded, &'static str>> {
        Some(self.encode_on(target, request))
    }
}

impl ConeStage {
    fn encode_on(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Result<ExternalEncoded, &'static str> {
        let (sw, sh) = request.surface_size;
        if sw == 0 || sh == 0 {
            return Err("cone stage has an empty surface");
        }
        let (kx, ky) = (
            target.size.0 as f32 / sw as f32,
            target.size.1 as f32 / sh as f32,
        );
        let mut state = self.state.lock().map_err(|_| "cone tiles poisoned")?;
        let atlas = atlas_for(&target.device);
        let mut atlas = atlas.lock().map_err(|_| "cone atlas poisoned")?;
        let tiles = if state.rects.is_some() {
            None
        } else {
            // The tiles at this target's shadow scale.
            let factor = d3d::scaled::shadow_scale(kx.round().max(1.0) as u32, Lighting::E3);
            let rects: Vec<[u32; 4]> = state
                .cones
                .iter()
                .map(|p| {
                    cone_shadows::slot_rect(p.slot, cone_shadows::tile_edge(factor, p.flashlight))
                })
                .collect();
            let work: Vec<(ConeTile, Vec<SunMesh>)> = state
                .cones
                .iter()
                .zip(&rects)
                .map(|(p, rect)| {
                    let meshes = p
                        .casters
                        .iter()
                        .filter_map(|c| upload(&target.device, c))
                        .collect();
                    (
                        ConeTile {
                            rect: *rect,
                            cone: p.cone,
                            strength: 1.0,
                        },
                        meshes,
                    )
                })
                .collect();
            state.rects = Some(rects);
            let pass = atlas.prepare_tiles(&target.device, &work);
            state.work.cones_drawn += work.len() as u32;
            state.work.tile_draws += pass.as_ref().map_or(0, |t| t.draws()) as u32;
            pass
        };
        let rect = state
            .rects
            .as_ref()
            .and_then(|r| r.get(self.cone).copied())
            .ok_or("cone stage has no tile")?;
        let planned = state.cones.get(self.cone).ok_or("cone stage has no cone")?;
        let tile = ConeTile {
            rect,
            cone: planned.cone,
            strength: 1.0,
        };
        let meshes: Vec<(SunMesh, [f32; 3], f32)> = planned
            .receivers
            .iter()
            .filter_map(|r| upload(&target.device, r).map(|m| (m, r.centre, r.radius)))
            .collect();
        let receivers: Vec<SunReceiver<'_>> = meshes
            .iter()
            .map(|(mesh, centre, radius)| SunReceiver {
                mesh,
                centre: *centre,
                radius: *radius,
            })
            .collect();
        let (gx, gy, gw, gh) = request.rect;
        let frame = SunFrame {
            viewport: Some([
                gx as f32 * kx,
                gy as f32 * ky,
                gw as f32 * kx,
                gh as f32 * ky,
            ]),
            ..self.frame
        };
        let formats = (target.color_format, target.depth_format);
        let lit = atlas.prepare_receivers(&target.device, formats, &frame, &tile, &receivers);
        state.work.receiver_draws += lit.as_ref().map_or(0, |r| r.draws()) as u32;
        let draws = tiles.as_ref().map_or(0, |t| t.draws()) + lit.as_ref().map_or(0, |r| r.draws());
        let passes = usize::from(tiles.is_some()) + usize::from(lit.is_some());
        let (colour, depth) = (target.color.clone(), target.depth.clone());
        Ok(ExternalEncoded {
            ranges: 1,
            draws,
            passes,
            commands: Box::new(move |encoder| {
                if let Some(t) = &tiles {
                    t.record(encoder);
                }
                if let Some(r) = &lit {
                    r.record(encoder, &colour, &depth);
                }
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Work sums as the stages record it, and a fresh replay reports none.
    #[test]
    fn a_replay_with_no_cone_reports_no_work() {
        let replay = ConeReplay {
            before: HashMap::new(),
            skip: HashSet::new(),
            state: Arc::new(Mutex::new(FrameCones {
                cones: Vec::new(),
                rects: None,
                work: ConeDraws {
                    cones_kept: 2,
                    ..ConeDraws::default()
                },
            })),
        };
        assert_eq!(
            replay.work(),
            ConeDraws {
                cones_kept: 2,
                ..ConeDraws::default()
            }
        );
    }
}
