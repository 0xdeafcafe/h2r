//! The executed attachment runs of an owned frame or private capture.
//! The private and displayed sources have independent armings.

use agent::compare::Rgba;
use agent::render::private_graph::GraphRecording;
use agent_proto::{
    CaptureLighting,
    frame_graph::{DisplayStats, FrameGraphStats},
};
use d3d::Lighting;
use d3d::backend::DrawOutcome;
use d3d::scene_sink::{ReplayFrame, ReplayOp};
use frame_scene::{FrameScene, TargetRef};
use native_render::frame_graph::{CapturedFrame, FrameReport, PassObserver, PassRow, ReadColour};

fn mode(lighting: Lighting) -> CaptureLighting {
    match lighting {
        Lighting::Retail => CaptureLighting::Retail,
        Lighting::E3 => CaptureLighting::E3,
    }
}

fn stats(scene: &FrameScene, present: u64, lighting: Lighting) -> FrameGraphStats {
    FrameGraphStats {
        map_epoch: scene.id.map_epoch,
        frame: scene.id.frame,
        tick: scene.id.tick,
        present,
        lighting: mode(lighting),
        // Filled only after a graph completes.
        scene_hash: String::new(),
        passes: 0,
        draws: 0,
        clears: 0,
        copies: 0,
        render_passes: 0,
        backend_draws: 0,
        submits: 0,
        e3: None,
    }
}

pub struct PrivateFrame<'a> {
    replay: &'a ReplayFrame,
    next_op: usize,
    graph: GraphRecording,
}

impl<'a> PrivateFrame<'a> {
    pub fn begin(
        scene: &FrameScene,
        replay: &'a ReplayFrame,
        present: u64,
        lighting: Lighting,
    ) -> Self {
        let graph = GraphRecording::begin(stats(scene, present, lighting));
        Self {
            replay,
            next_op: 0,
            graph,
        }
    }

    /// Copies are graph barriers. Attribute them to the draw count at the
    /// barrier, including copies before the first or after the last pass.
    fn between(&mut self, end: usize) {
        for (_, op) in &self.replay.ops[self.next_op..end] {
            if let ReplayOp::Copy { from, to } = op {
                self.graph.copy(|| (format!("{from:?}"), format!("{to:?}")));
            }
        }
        self.next_op = end;
    }

    pub fn completed(mut self, output: &CapturedFrame) {
        self.between(self.replay.ops.len());
        self.finish_graph(output.scene_hash, &output.report);
    }

    fn finish_graph(self, hash: u64, report: &FrameReport) {
        self.graph.complete(
            hash,
            report.render_passes,
            report.draws,
            report.submits,
            super::e3::e3_report(report),
        );
    }
}

impl PassObserver for PrivateFrame<'_> {
    fn pass_end(
        &mut self,
        index: usize,
        row: &PassRow,
        read_colour: &mut dyn FnMut() -> ReadColour,
    ) {
        self.between(row.ops.start);
        let depth = (row.depth != TargetRef::None).then_some(row.depth);
        self.graph.pass(
            || format!("{:?}", row.colour),
            || depth.map(|s| format!("{s:?}")),
        );
        for (at, op) in &self.replay.ops[row.ops.clone()] {
            match op {
                ReplayOp::Draw { draw, .. } => {
                    self.graph.slot(at.view, at.slot.name());
                    self.graph.draw(|| {
                        crate::d3d_diag::probe_draw(
                            row.colour,
                            depth,
                            draw,
                            &Ok(DrawOutcome::Drawn),
                        )
                    });
                }
                ReplayOp::Clear { .. } | ReplayOp::ClearRects { .. } => {
                    self.graph.slot(at.view, at.slot.name());
                    self.graph.clear();
                }
                ReplayOp::Stage(stage) => {
                    self.graph.slot(at.view, at.slot.name());
                    self.graph.stage(stage.plan.stage.name());
                }
                _ => {}
            }
        }
        self.next_op = row.ops.end;
        self.graph.snapshot(index, || {
            let ([width, height], pixels) = read_colour()?;
            Ok(Rgba {
                width,
                height,
                pixels,
            })
        });
    }
}

/// Observe the chosen native display graph only when the matching frame
/// was armed. Unarmed frames publish just the backend counter deltas.
pub struct OwnedFrame<'a> {
    recorder: Option<PrivateFrame<'a>>,
    scene: &'a FrameScene,
    present: u64,
}

impl<'a> OwnedFrame<'a> {
    pub fn begin(
        scene: &'a FrameScene,
        replay: &'a ReplayFrame,
        present: u64,
        lighting: Lighting,
    ) -> Self {
        let recorder = GraphRecording::begin_display(present, || stats(scene, present, lighting))
            .map(|graph| PrivateFrame {
                replay,
                next_op: 0,
                graph,
            });
        Self {
            recorder,
            scene,
            present,
        }
    }

    pub fn observer(&mut self) -> Option<&mut dyn PassObserver> {
        self.recorder.as_mut().map(|p| p as &mut dyn PassObserver)
    }

    pub fn presented(self, report: &FrameReport) {
        if let Some(mut recording) = self.recorder {
            recording.between(recording.replay.ops.len());
            recording.finish_graph(frame_scene::summary_hash(self.scene), report);
        }
        agent::render::private_graph::display_presented(DisplayStats {
            map_epoch: report.id.map_epoch,
            frame: report.id.frame,
            tick: report.id.tick,
            present: self.present,
            lighting: mode(report.lighting),
            render_passes: report.render_passes,
            backend_draws: report.draws,
            submits: report.submits,
            e3: super::e3::e3_report(report),
        });
    }
}
