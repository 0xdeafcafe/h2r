//! A game view rendered from the scene sealed at its present (FrameScene
//! slice 6). The sink records guest work at its original position, including
//! copies and reads of intermediate render targets. A slot boundary alone
//! never ends an attachment pass: two slots can draw into the same open
//! pass. Display and capture use the same recording path and separate
//! attachments. Only display hands its final target to the presenter.

mod lighting;
mod private;
pub use lighting::{E3Report, PassWork};

use std::ops::Range;

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

/// `render.frame`'s logical pass boundaries. Build them only when a
/// recorder is armed. Ordinary frames simply stream their ops to the
/// encoder, whose pending draw list merges adjacent draws.
fn pass_rows(frame: &ReplayFrame) -> Vec<PassRow> {
    let mut rows = Vec::new();
    let mut current: Option<PassRow> = None;
    let mut bound = None;
    for (i, (_, op)) in frame.ops.iter().enumerate() {
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
}

impl FramePass for GuestDrawPass<'_> {
    fn apply(&mut self, op: &ReplayOp) -> Result<(), String> {
        GuestDrawPass::apply(self, op)
    }
    fn read_target(&mut self, target: TargetRef) -> ReadColour {
        GuestDrawPass::read_target(self, target)
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

fn record(
    pass: &mut impl FramePass,
    frame: &ReplayFrame,
    fresh_targets: bool,
    lighting: Lighting,
    observer: Option<&mut dyn PassObserver>,
) -> Result<Option<E3Report>, String> {
    // The graph replays the guest's layers in both modes. Only work
    // submitted by the E3 stages belongs to these four pass rows.
    let e3 = (lighting == Lighting::E3).then(E3Report::default);
    let rows = observer
        .as_ref()
        .map(|_| pass_rows(frame))
        .unwrap_or_default();
    let mut rows = rows.into_iter().enumerate().peekable();
    let mut observer = observer;
    for (i, (at, op)) in frame.ops.iter().enumerate() {
        pass.apply(op)
            .map_err(|e| format!("view {} {} op {i}: {e}", at.view, at.slot.name()))?;
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
    Ok(e3)
}

/// Re-render this scene to an isolated target. No present hook or
/// display settings are touched. At L3 both lighting modes run the same
/// pass sequence.
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
    let mut pass = GuestDrawPass::new(backend, replay, scale)?;
    let e3 = record(&mut pass, replay, true, lighting, observer)?;
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
#[allow(clippy::too_many_arguments)]
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
    let mut pass = GuestDrawPass::new_display(backend, replay, scale)?;
    pass.set_lighting(lighting);
    let e3 = record(&mut pass, replay, true, lighting, observer)?;
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
/// private draws always allocate their own.
#[allow(clippy::too_many_arguments)]
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
    let mut pass = GuestDrawPass::on_existing(backend, replay, targets)?;
    pass.set_lighting(lighting);
    let e3 = record(&mut pass, replay, false, lighting, observer)?;
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
    let e3 = record(&mut pass, replay, false, lighting, observer)?;
    let (after, outputs) = pass.present(replay.flags, replay.gamma.as_deref())?;
    Ok((
        FrameReport::new(scene.id, lighting, e3, before, after),
        outputs,
    ))
}
