//! A game view rendered from the scene sealed at its present (FrameScene
//! slice 6). The sink records guest work at its original position, including
//! copies and reads of intermediate render targets. A slot boundary alone
//! never ends an attachment pass: two slots can draw into the same open
//! pass. Display and capture use the same recording path and separate
//! attachments. Only display hands its final target to the presenter.

mod inputs;
pub use inputs::{freeze_display_inputs, freeze_scaled_inputs};
mod lighting;
mod markers;
mod private;
mod sun;
pub use lighting::{E3Report, PassWork};
pub use sun::{RECEIVER_DEPTH_BIAS, SunSkip};

use std::ops::Range;

use d3d::gfx_backend::TileWork;
use d3d::gfx_backend::{BatchStats, GuestDrawPass};
use d3d::scaled::{ScaledBackend, ScaledScenePass, ScaledSurface, ScaledTexture};
use d3d::scene_sink::{ReplayFrame, ReplayOp};
use d3d::threaded::owner::Outputs;
use d3d::{GfxBackend, Lighting};
use frame_scene::{FrameScene, SceneId, TargetRef, summary_hash};

/// One run of draws and clears on the same attachments. The operations
/// retain their original slot and view in `ReplayFrame::ops`. A return to
/// a target after a copy starts a new row. A logical row can encode more
/// than one hardware pass, for example for a pixel-count query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassRow {
    pub colour: TargetRef,
    pub depth: TargetRef,
    pub ops: Range<usize>,
}

pub type ReadColour = Result<([u32; 2], Vec<u8>), String>;

/// Called at the end of the row, before any later op runs. Only an
/// observer that actually invokes the closure reads pixels back.
pub trait PassObserver {
    fn pass_end(
        &mut self,
        index: usize,
        row: &PassRow,
        read_colour: &mut dyn FnMut() -> ReadColour,
    );
}

/// Submitted work for this draw of the sealed scene. A private capture
/// returns its own report and does not publish it as a displayed frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameReport {
    pub id: SceneId,
    pub lighting: Lighting,
    /// Present for a completed draw using the E3 mode.
    pub e3: Option<E3Report>,
    pub render_passes: u64,
    pub draws: u64,
    pub submits: u64,
}

impl FrameReport {
    fn new(
        id: SceneId,
        lighting: Lighting,
        e3: Option<E3Report>,
        before: BatchStats,
        after: BatchStats,
    ) -> Self {
        Self {
            id,
            lighting,
            e3,
            render_passes: after.render_passes - before.render_passes,
            draws: after.draws - before.draws,
            submits: after.submits - before.submits,
        }
    }
}

/// Pixels returned by the private draw. The tick and hash come from the
/// same immutable scene whose commands produced these pixels.
pub struct CapturedFrame {
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
    pub id: SceneId,
    pub scene_hash: u64,
    pub report: FrameReport,
}

/// Logical pass cuts shared by input preservation and armed observers.
/// Recording streams ops to the encoder, whose pending list merges
/// adjacent draws on compatible attachments.
fn pass_rows(frame: &ReplayFrame) -> Vec<PassRow> {
    let mut rows = Vec::new();
    let mut current: Option<PassRow> = None;
    let mut bound = None;
    for (i, (_, op)) in frame.ops.iter().enumerate() {
        // An object shadow's mark draws nothing and does not cut a row.
        if op.is_marker() {
            continue;
        }
        let attachments = match op {
            ReplayOp::Bind { colour, depth } => {
                bound = Some((*colour, *depth));
                // A repeated bind at a slot transition contributes no work.
                continue;
            }
            ReplayOp::Draw { .. } => bound,
            ReplayOp::Stage(stage) => Some((stage.surfaces.color, stage.surfaces.depth)),
            ReplayOp::Clear { colour, depth, .. } | ReplayOp::ClearRects { colour, depth, .. } => {
                Some((*colour, *depth))
            }
            // Transfers, samples and queries must see all preceding work.
            _ => None,
        };
        if let Some((colour, depth)) = attachments {
            if let Some(row) = current.as_mut()
                && (colour, depth) == (row.colour, row.depth)
            {
                row.ops.end = i + 1;
                continue;
            }
            if let Some(row) = current.replace(PassRow {
                colour,
                depth,
                ops: i..i + 1,
            }) {
                rows.push(row);
            }
        } else if let Some(row) = current.take() {
            rows.push(row);
        }
    }
    rows.extend(current);
    rows
}

trait FramePass {
    fn apply(&mut self, op: &ReplayOp) -> Result<(), String>;
    fn read_target(&mut self, target: TargetRef) -> ReadColour;
    /// E3's shadow tiles, on a pass that draws them.
    fn tile_work(&self) -> Option<TileWork> {
        None
    }
}

impl FramePass for GuestDrawPass<'_> {
    fn apply(&mut self, op: &ReplayOp) -> Result<(), String> {
        GuestDrawPass::apply(self, op)
    }
    fn read_target(&mut self, target: TargetRef) -> ReadColour {
        GuestDrawPass::read_target(self, target)
    }
    fn tile_work(&self) -> Option<TileWork> {
        GuestDrawPass::tile_work(self)
    }
}

impl FramePass for ScaledScenePass<'_> {
    fn apply(&mut self, op: &ReplayOp) -> Result<(), String> {
        ScaledScenePass::apply(self, op)
    }
    fn read_target(&mut self, target: TargetRef) -> ReadColour {
        ScaledScenePass::read_target(self, target)
    }
}

/// E3's sun tiles for one draw of `replay` on `device`
/// ([`sun`]): `None` in any other mode, so retail replays every op.
fn sun_tiles(
    device: &wgpu::Device,
    scene: &FrameScene,
    replay: &ReplayFrame,
    scale: u32,
    lighting: Lighting,
) -> Option<sun::SunReplay> {
    if lighting != Lighting::E3 {
        return None;
    }
    let models = crate::sun_casters::models_for(scene.id.map_epoch);
    let models = models
        .as_deref()
        .map(|m| m as &dyn crate::sun_casters::CasterModels);
    let world = crate::sun_casters::world_for(scene.id.map_epoch);
    let world = world
        .as_deref()
        .map(|w| w as &dyn crate::sun_casters::WorldReceivers);
    sun::plan(device, scene, replay, scale, models, world)
}

fn record(
    pass: &mut impl FramePass,
    scene: &FrameScene,
    frame: &ReplayFrame,
    fresh_targets: bool,
    lighting: Lighting,
    observer: Option<&mut dyn PassObserver>,
    sun_replay: Option<sun::SunReplay>,
) -> Result<Option<E3Report>, String> {
    // The graph replays the guest's layers in both modes. Only work
    // submitted by the E3 stages belongs to these four pass rows.
    let mut e3 = (lighting == Lighting::E3).then(E3Report::default);
    // The marker skin: a stage after each game view's last op, before the
    // overlay (`markers`).
    let marker_stages = markers::plan(scene, frame);
    let rows = observer
        .as_ref()
        .map(|_| pass_rows(frame))
        .unwrap_or_default();
    let mut rows = rows.into_iter().enumerate().peekable();
    let mut observer = observer;
    let ran = (|| -> Result<(), String> {
        for (i, (at, op)) in frame.ops.iter().enumerate() {
            let named = |e: String| format!("view {} {} op {i}: {e}", at.view, at.slot.name());
            // An admitted object shadow's ops: its sun stage stands in.
            if !sun_replay.as_ref().is_some_and(|s| s.skip[i]) {
                pass.apply(op).map_err(named)?;
            }
            if let Some(stage) = sun_replay.as_ref().and_then(|s| s.after.get(&i)) {
                pass.apply(stage).map_err(named)?;
            }
            if let Some(stage) = marker_stages.get(&i) {
                pass.apply(stage).map_err(named)?;
            }
            if let Some((index, row)) = rows.next_if(|(_, row)| row.ops.end == i + 1)
                && let Some(observer) = observer.as_deref_mut()
            {
                observer.pass_end(index, &row, &mut || {
                    if fresh_targets
                        && let Some(reason) = frame.private_read_refusal(row.ops.end, row.colour)
                    {
                        return Err(reason.into());
                    }
                    pass.read_target(row.colour)
                });
            }
        }
        Ok(())
    })();
    if let Some(s) = &sun_replay {
        s.finish(ran.is_ok());
    }
    ran?;
    if let (Some(report), Some(tiles)) = (e3.as_mut(), pass.tile_work()) {
        report.add_tiles(tiles);
    }
    if let (Some(report), Some(s)) = (e3.as_mut(), &sun_replay) {
        report.add_sun(s.work());
    }
    if let Some(report) = e3.as_mut() {
        report.add_choice(&crate::light_choice::choose(scene));
    }
    Ok(e3)
}

/// Re-render this scene to an isolated target. No present hook or
/// display settings are touched. E3 draws the guest's shadow maps as
/// tiles and compares their receivers with the Sharp edge
/// (`d3d::gfx_backend::guest_draw_pass::tiles`); retail replays the
/// guest's passes as they are.
pub fn draw_private(
    backend: &mut GfxBackend,
    scene: &FrameScene,
    replay: &ReplayFrame,
    scale: u32,
    lighting: Lighting,
) -> Result<CapturedFrame, String> {
    draw_private_with_observer(backend, scene, replay, scale, lighting, None)
}

pub fn draw_private_with_observer(
    backend: &mut GfxBackend,
    scene: &FrameScene,
    replay: &ReplayFrame,
    scale: u32,
    lighting: Lighting,
    observer: Option<&mut dyn PassObserver>,
) -> Result<CapturedFrame, String> {
    if scale > 1 && replay.has_mixed_target_sizes() {
        return private::draw_mixed(backend, scene, replay, scale, lighting, observer);
    }
    let before = backend.batch_stats();
    let sun_replay = sun_tiles(&backend.ctx.device, scene, replay, scale, lighting);
    let mut pass = GuestDrawPass::new_lit(backend, replay, scale, lighting)?;
    let e3 = record(&mut pass, scene, replay, true, lighting, observer, sun_replay)?;
    let (size, rgba) = pass.read_rgba()?;
    Ok(CapturedFrame {
        id: scene.id,
        scene_hash: summary_hash(scene),
        size,
        rgba,
        report: FrameReport::new(scene.id, lighting, e3, before, backend.batch_stats()),
    })
}

/// Present the result of the same graph at the display scale. Logical
/// guest ordering, including split-screen view transitions, is preserved.
pub fn draw_display(
    backend: &mut GfxBackend,
    scene: &FrameScene,
    replay: &ReplayFrame,
    scale: u32,
    lighting: Lighting,
    flags: u32,
    gamma: Option<&[u8; 768]>,
    observer: Option<&mut dyn PassObserver>,
) -> Result<FrameReport, String> {
    let before = backend.batch_stats();
    let sun_replay = sun_tiles(&backend.ctx.device, scene, replay, scale, lighting);
    let mut pass = GuestDrawPass::new_display_lit(backend, replay, scale, lighting)?;
    let e3 = record(&mut pass, scene, replay, true, lighting, observer, sun_replay)?;
    pass.present(flags, gamma)?;
    Ok(FrameReport::new(
        scene.id,
        lighting,
        e3,
        before,
        backend.batch_stats(),
    ))
}

/// The 1x display path can use the renderer's live attachments, including
/// their contents from the preceding frame. They belong to the caller;
/// private draws always allocate their own. Its shadow maps keep the
/// guest's size, so it draws no E3 tiles.
pub fn draw_display_on(
    backend: &mut GfxBackend,
    scene: &FrameScene,
    replay: &ReplayFrame,
    lighting: Lighting,
    flags: u32,
    gamma: Option<&[u8; 768]>,
    observer: Option<&mut dyn PassObserver>,
    targets: &std::collections::HashMap<TargetRef, gfx::surface::SurfaceId>,
) -> Result<(FrameReport, Outputs<d3d::gfx_backend::HostTexture>), String> {
    let before = backend.batch_stats();
    // Failure belongs to private replay. The live graph still owns its
    // original attachments and can complete this display.
    let _ = freeze_display_inputs(backend, replay, targets);
    let sun_replay = sun_tiles(&backend.ctx.device, scene, replay, 1, lighting);
    let mut pass = GuestDrawPass::on_existing(backend, replay, targets)?;
    pass.set_lighting(lighting);
    let e3 = record(&mut pass, scene, replay, false, lighting, observer, sun_replay)?;
    let outputs = pass.present_with_outputs(flags, gamma)?;
    Ok((
        FrameReport::new(scene.id, lighting, e3, before, backend.batch_stats()),
        outputs,
    ))
}

/// Present on the scaled wrapper's authoritative images even when a row
/// pairs a guest-size auxiliary colour level with the main scaled depth.
pub fn draw_scaled_display_on(
    backend: &mut ScaledBackend<GfxBackend, GfxBackend>,
    scene: &FrameScene,
    replay: &ReplayFrame,
    observer: Option<&mut dyn PassObserver>,
    targets: &std::collections::HashMap<
        TargetRef,
        ScaledSurface<gfx::surface::SurfaceId, gfx::surface::SurfaceId>,
    >,
) -> Result<
    (
        FrameReport,
        Outputs<ScaledTexture<d3d::gfx_backend::HostTexture, d3d::gfx_backend::HostTexture>>,
    ),
    String,
> {
    let lighting = backend.lighting();
    let before = backend.view().batch_stats();
    let mut pass = ScaledScenePass::new(backend, replay, targets)?;
    let _ = inputs::freeze_native_scaled_inputs(&mut pass, replay);
    // The scaled display draws no sun tile: every object shadow keeps
    // retail's mask there.
    let e3 = record(&mut pass, scene, replay, false, lighting, observer, None)?;
    let (after, outputs) = pass.present(replay.flags, replay.gamma.as_deref())?;
    Ok((
        FrameReport::new(scene.id, lighting, e3, before, after),
        outputs,
    ))
}
