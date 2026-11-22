//! E3's sun tiles in the frame graph (docs/graphics/e3-lighting.md, "Sun
//! shadows from objects"). The scene sink marks each object shadow's ops
//! (`ReplayOp::ObjectShadow`). Under E3, an object shadow whose caster and
//! receivers the scene can draw, and whose tile the planner places, has
//! its marked ops skipped: retail's mask chain is not drawn for it. A sun
//! stage at the end of its ops draws its receivers against its tile, on
//! the scene's colour and depth, at the guest's W-buffer depth. The first
//! such stage also draws the frame's tiles into the atlas. Every other
//! shadow keeps its ops and retail's mask, and its cause is named
//! ([`SunSkip`]). Retail never plans any of this.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use d3d::backend::{
    ExternalAttachments, ExternalPass, ExternalPassRequest, ExternalPlan, ExternalSurfaces,
    SceneStage,
};
use d3d::scene_sink::{ReplayFrame, ReplayOp, ReplayTexture};
use frame_scene::{D24_SCALE, FrameScene, TargetRef};
use gfx::D3DFormat;
use gfx::external::ExternalEncoded;

use crate::sun_casters::{self, CasterModels, CasterRefusal, ReceiverMesh};
use crate::sun_tiles::{ATLAS_EDGE, SunCaster, SunPlan, SunWork};
use crate::sun_tiles_gpu::{SunFrame, SunMesh, SunReceiver, SunShadows};

/// How far towards the eye a receiver meets the guest's depth, in view
/// depth (world units): the guest skins and projects its own vertices,
/// so a receiver's depth differs from the depth it laid by rounding.
pub const RECEIVER_DEPTH_BIAS: f32 = 0.005;

/// Why an object shadow keeps retail's mask under E3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SunSkip {
    /// No caster models are loaded for the scene's map.
    NoModels,
    /// The caster or its receivers are refused (`sun_casters`).
    Caster(CasterRefusal),
    /// Its ops are marked more than once in the frame.
    Split,
    /// Its marked ops draw nothing on the scene's colour and depth, or its
    /// receivers do not draw there.
    Attachments,
    /// A target its ops write, or a sample they take, is read after them.
    Escapes,
    /// The scene's depth is not a 24-bit W-buffer the receivers can meet.
    DepthFormat,
    /// Its view's viewport is empty.
    Viewport,
    /// The planner gave its caster no tile.
    NoTile,
}

/// What one draw of the frame does about the sun.
pub(crate) struct SunReplay {
    /// Ops not applied: the admitted shadows' marked ops, binds kept.
    pub skip: Vec<bool>,
    /// A sun stage to apply after the op at each index.
    pub after: HashMap<usize, ReplayOp>,
    /// Every marked shadow that keeps retail's mask, by scene index. Not
    /// reported yet: the E3 report counts only the planner's refusals.
    #[allow(dead_code)]
    pub kept: Vec<(usize, SunSkip)>,
    state: Option<Arc<Mutex<FrameSun>>>,
}

impl SunReplay {
    /// Every marked shadow keeps its ops.
    fn keep(frame: &ReplayFrame, kept: Vec<(usize, SunSkip)>) -> Self {
        Self {
            skip: vec![false; frame.ops.len()],
            after: HashMap::new(),
            kept,
            state: None,
        }
    }

    /// After the draw's ops, `recorded` when they all applied. The atlas
    /// is invalidated when the planner's tiles may not have been drawn:
    /// no stage recorded them, or the draw failed after one did.
    pub fn finish(&self, recorded: bool) {
        let Some(state) = &self.state else { return };
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        if !recorded || !state.tiles_drawn {
            state
                .shadows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .invalidate();
        }
    }

    /// The sun's work this draw, once its stages have recorded.
    pub fn work(&self) -> SunWork {
        self.state
            .as_ref()
            .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).plan.work)
            .unwrap_or_default()
    }
}

/// The frame's tiles, shared by its sun stages in op order.
struct FrameSun {
    shadows: Arc<Mutex<SunShadows>>,
    plan: SunPlan,
    /// Each tiled caster's mesh, by caster id.
    casters: Vec<(u64, SunMesh)>,
    tiles_drawn: bool,
}

/// Each device's atlas and planner, kept across frames.
static SHADOWS: Mutex<Vec<(wgpu::Device, Arc<Mutex<SunShadows>>)>> = Mutex::new(Vec::new());

fn shadows_for(device: &wgpu::Device) -> Arc<Mutex<SunShadows>> {
    let mut all = SHADOWS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, s)) = all.iter().find(|(d, _)| d == device) {
        return Arc::clone(s);
    }
    let made = Arc::new(Mutex::new(SunShadows::new(device, ATLAS_EDGE)));
    all.push((device.clone(), Arc::clone(&made)));
    made
}

/// One marked object shadow: the ops between its mark and the next.
struct Marked {
    shadow: usize,
    ops: std::ops::Range<usize>,
}

fn marked(frame: &ReplayFrame) -> Vec<Marked> {
    let mut out = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    for (i, (_, op)) in frame.ops.iter().enumerate() {
        if let ReplayOp::ObjectShadow(next) = op {
            if let Some((shadow, start)) = open.take() {
                out.push(Marked {
                    shadow,
                    ops: start..i,
                });
            }
            open = next.map(|s| (s as usize, i + 1));
        }
    }
    if let Some((shadow, start)) = open {
        out.push(Marked {
            shadow,
            ops: start..frame.ops.len(),
        });
    }
    out
}

/// The attachments bound at each op, by a pass over the frame.
fn bound_at(frame: &ReplayFrame) -> Vec<Option<(TargetRef, TargetRef)>> {
    let mut bound = None;
    frame
        .ops
        .iter()
        .map(|(_, op)| {
            if let ReplayOp::Bind { colour, depth } = op {
                bound = Some((*colour, *depth));
            }
            bound
        })
        .collect()
}

/// The marked ops of a shadow draw last on the scene's colour and depth,
/// and nothing they write or sample, other than the scene's colour and
/// depth, is read after them.
fn check_ops(
    frame: &ReplayFrame,
    bound: &[Option<(TargetRef, TargetRef)>],
    ops: &std::ops::Range<usize>,
) -> Result<(), SunSkip> {
    let scene = [TargetRef::SceneColour, TargetRef::SceneDepth];
    let last_draw = ops
        .clone()
        .rev()
        .find(|&i| matches!(frame.ops[i].1, ReplayOp::Draw { .. }))
        .ok_or(SunSkip::Attachments)?;
    if bound[last_draw] != Some((scene[0], scene[1])) {
        return Err(SunSkip::Attachments);
    }
    let mut samples = HashSet::new();
    let mut written = HashSet::new();
    for i in ops.clone() {
        match &frame.ops[i].1 {
            ReplayOp::Sample { id, .. } => {
                samples.insert(*id);
            }
            ReplayOp::Draw { .. } => {
                if let Some((colour, depth)) = bound[i] {
                    written.extend([colour, depth]);
                }
            }
            ReplayOp::Clear { colour, depth, .. } | ReplayOp::ClearRects { colour, depth, .. } => {
                written.extend([*colour, *depth]);
            }
            ReplayOp::Copy { to, .. } | ReplayOp::Upload { to, .. } => {
                written.insert(*to);
            }
            ReplayOp::Stage(stage) => {
                written.extend([stage.surfaces.color, stage.surfaces.depth]);
            }
            _ => {}
        }
    }
    written.retain(|t| *t != TargetRef::None && !scene.contains(t));
    let read_after = |t: &ReplayTexture| match t {
        ReplayTexture::Sample(id) => samples.contains(id),
        ReplayTexture::Live { target, .. } => written.contains(target),
        _ => false,
    };
    let escapes = frame.ops[ops.end..].iter().any(|(_, op)| match op {
        ReplayOp::Draw { textures, .. } => textures.iter().flatten().any(read_after),
        ReplayOp::Copy { from, .. } => written.contains(from),
        ReplayOp::Sample { of, .. } => written.contains(of),
        ReplayOp::Stage(stage) => stage.surfaces.sampled.iter().any(|t| written.contains(t)),
        _ => false,
    });
    if escapes {
        return Err(SunSkip::Escapes);
    }
    Ok(())
}

/// Whether the frame graph applies `op` inside an admitted shadow's ops:
/// binds and samples keep the replay's state and sample ids whole.
fn kept_inside(op: &ReplayOp) -> bool {
    matches!(
        op,
        ReplayOp::Bind { .. }
            | ReplayOp::Sample { .. }
            | ReplayOp::BeginFlareQuery
            | ReplayOp::EndFlareQuery(_)
            | ReplayOp::ObjectShadow(_)
    )
}

/// World to clip as the sun program's rows: the camera's column-major
/// view-projection, transposed.
fn clip_rows(m: &frame_scene::Mat4) -> [[f32; 4]; 4] {
    let mut rows = [[0.0; 4]; 4];
    for (r, row) in rows.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = m[c][r];
        }
    }
    rows
}

/// Plans the sun for one E3 draw of `frame` from `scene` at `scale` on
/// `device`, with `models` the scene's map's caster models. `None` when
/// the frame marks no object shadow.
pub(crate) fn plan(
    device: &wgpu::Device,
    scene: &FrameScene,
    frame: &ReplayFrame,
    scale: u32,
    models: Option<&dyn CasterModels>,
) -> Option<SunReplay> {
    let marks = marked(frame);
    if marks.is_empty() {
        return None;
    }
    let mut kept = Vec::new();
    let Some(models) = models else {
        kept.extend(marks.iter().map(|m| (m.shadow, SunSkip::NoModels)));
        return Some(SunReplay::keep(frame, kept));
    };
    let mut seen = HashMap::<usize, usize>::new();
    for m in &marks {
        *seen.entry(m.shadow).or_default() += 1;
    }
    let depth_ok = frame.depth.is_some_and(|d| {
        d.format == D3DFormat::D24S8 as u32 || d.format == D3DFormat::LinD24S8 as u32
    });
    let bound = bound_at(frame);
    // Each view's casters, gathered once.
    let mut gathered = HashMap::new();
    let mut accepted: Vec<(
        &Marked,
        SunCaster,
        Vec<ReceiverMesh>,
        sun_casters::CasterMesh,
    )> = Vec::new();
    for m in &marks {
        let checked =
            (|| -> Result<(SunCaster, Vec<ReceiverMesh>, sun_casters::CasterMesh), SunSkip> {
                if seen[&m.shadow] > 1 {
                    return Err(SunSkip::Split);
                }
                if !depth_ok {
                    return Err(SunSkip::DepthFormat);
                }
                let o = scene
                    .object_shadows
                    .get(m.shadow)
                    .ok_or(SunSkip::Caster(CasterRefusal::NoInstance))?;
                let v = scene
                    .views
                    .get(o.view as usize)
                    .ok_or(SunSkip::Caster(CasterRefusal::NoInstance))?;
                let [top, _, bottom, _] = v.camera.viewport;
                let height = u32::try_from(i32::from(bottom) - i32::from(top))
                    .ok()
                    .filter(|&h| h > 0)
                    .ok_or(SunSkip::Viewport)?;
                let got = gathered
                    .entry(o.view)
                    .or_insert_with(|| sun_casters::gather(scene, o.view, height * scale, models));
                if let Some((_, cause)) = got.refused.iter().find(|(s, _)| *s == m.shadow) {
                    return Err(SunSkip::Caster(*cause));
                }
                let admitted = got
                    .admitted
                    .iter()
                    .find(|a| a.shadow == m.shadow)
                    .ok_or(SunSkip::Caster(CasterRefusal::NoInstance))?;
                let on =
                    sun_casters::receivers(scene, m.shadow, models).map_err(SunSkip::Caster)?;
                check_ops(frame, &bound, &m.ops)?;
                Ok((admitted.caster, on, admitted.mesh.clone()))
            })();
        match checked {
            Ok((caster, on, mesh)) => accepted.push((m, caster, on, mesh)),
            Err(cause) => kept.push((m.shadow, cause)),
        }
    }
    if accepted.is_empty() {
        return Some(SunReplay::keep(frame, kept));
    }
    let shadows = shadows_for(device);
    let casters: Vec<SunCaster> = accepted.iter().map(|a| a.1).collect();
    let planned = shadows
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .plan(&casters);
    let tiled: HashSet<u64> = planned.tiles.iter().map(|t| t.id).collect();
    let state = Arc::new(Mutex::new(FrameSun {
        shadows,
        plan: planned,
        casters: Vec::new(),
        tiles_drawn: false,
    }));
    let mut skip = vec![false; frame.ops.len()];
    let mut after = HashMap::new();
    for (m, caster, on, mesh) in accepted {
        match sun_stage(device, scene, frame, m, &caster, &on, &mesh, &tiled, &state) {
            Ok(stage) => {
                for i in m.ops.clone() {
                    skip[i] = !kept_inside(&frame.ops[i].1);
                }
                after.insert(m.ops.end - 1, stage);
            }
            Err(cause) => kept.push((m.shadow, cause)),
        }
    }
    Some(SunReplay {
        skip,
        after,
        kept,
        state: Some(state),
    })
}

/// The sun stage that stands in for marked shadow `m`'s ops.
fn sun_stage(
    device: &wgpu::Device,
    scene: &FrameScene,
    frame: &ReplayFrame,
    m: &Marked,
    caster: &SunCaster,
    on: &[ReceiverMesh],
    mesh: &sun_casters::CasterMesh,
    tiled: &HashSet<u64>,
    state: &Arc<Mutex<FrameSun>>,
) -> Result<ReplayOp, SunSkip> {
    if !tiled.contains(&caster.id) {
        return Err(SunSkip::NoTile);
    }
    let o = &scene.object_shadows[m.shadow];
    let camera = &scene.views[o.view as usize].camera;
    let [top, left, bottom, right] = camera.viewport.map(|x| u32::try_from(x).unwrap_or(0));
    let width = right
        .checked_sub(left)
        .filter(|&w| w > 0)
        .ok_or(SunSkip::Viewport)?;
    let height = bottom
        .checked_sub(top)
        .filter(|&h| h > 0)
        .ok_or(SunSkip::Viewport)?;
    let colour = frame.colour.ok_or(SunSkip::Attachments)?;
    let empty = SunSkip::Caster(CasterRefusal::Empty);
    let caster_mesh =
        SunMesh::new(device, &mesh.positions, &mesh.normals, &mesh.indices).map_err(|_| empty)?;
    let mut receivers = Vec::with_capacity(on.len());
    for r in on {
        let g = SunMesh::new(device, &r.mesh.positions, &r.mesh.normals, &r.mesh.indices)
            .map_err(|_| empty)?;
        receivers.push((g, r.centre, r.radius));
    }
    state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .casters
        .push((caster.id, caster_mesh));
    let pass = SunStage {
        state: Arc::clone(state),
        tile: caster.id,
        receivers,
        frame: SunFrame {
            clip: clip_rows(&camera.view_projection()),
            depth: [D24_SCALE, 1.0 / camera.far, 1.0, RECEIVER_DEPTH_BIAS],
            viewport: None,
        },
    };
    Ok(ReplayOp::Stage(SceneStage {
        surfaces: ExternalSurfaces {
            color: TargetRef::SceneColour,
            depth: TargetRef::SceneDepth,
            sampled: vec![],
            slots: vec![],
            slot_sizes: vec![],
        },
        request: ExternalPassRequest {
            rect: (left, top, width, height),
            surface_size: (colour.width, colour.height),
            display_projection_factor: 1.0,
            z_scale: D24_SCALE,
        },
        plan: ExternalPlan {
            stage: timeline::native::Stage::Shadows,
            view: o.view,
            ranges: 1,
            slots: vec![],
        },
        pass: Arc::new(Mutex::new(Box::new(pass) as Box<dyn ExternalPass>)),
    }))
}

/// One shadow's receivers against its tile; the frame's first also draws
/// the tiles.
struct SunStage {
    state: Arc<Mutex<FrameSun>>,
    tile: u64,
    receivers: Vec<(SunMesh, [f32; 3], f32)>,
    frame: SunFrame,
}

impl ExternalPass for SunStage {
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

impl SunStage {
    fn encode_on(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Result<ExternalEncoded, &'static str> {
        let mut state = self.state.lock().map_err(|_| "sun tiles poisoned")?;
        let FrameSun {
            shadows,
            plan: frame_plan,
            casters,
            tiles_drawn,
        } = &mut *state;
        let mut shadows = shadows.lock().map_err(|_| "sun atlas poisoned")?;
        let tiles = if *tiles_drawn {
            None
        } else {
            let meshes: Vec<(u64, &SunMesh)> = casters.iter().map(|(id, m)| (*id, m)).collect();
            let pass = shadows.prepare_tiles(&target.device, frame_plan, &meshes);
            *tiles_drawn = true;
            pass.map_err(|_| "a sun tile has no caster mesh")?
        };
        let (gx, gy, gw, gh) = request.rect;
        let (sw, sh) = request.surface_size;
        if sw == 0 || sh == 0 {
            shadows.invalidate();
            return Err("sun stage has an empty surface");
        }
        let (kx, ky) = (
            target.size.0 as f32 / sw as f32,
            target.size.1 as f32 / sh as f32,
        );
        let frame = SunFrame {
            viewport: Some([
                gx as f32 * kx,
                gy as f32 * ky,
                gw as f32 * kx,
                gh as f32 * ky,
            ]),
            ..self.frame
        };
        let receivers: Vec<SunReceiver<'_>> = self
            .receivers
            .iter()
            .map(|(mesh, centre, radius)| SunReceiver {
                mesh,
                centre: *centre,
                radius: *radius,
            })
            .collect();
        let formats = (target.color_format, target.depth_format);
        let lit = shadows.prepare_receivers(
            &target.device,
            formats,
            &frame,
            frame_plan,
            Some(self.tile),
            &receivers,
        );
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
