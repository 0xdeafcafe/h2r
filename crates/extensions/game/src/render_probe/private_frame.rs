//! The actual graph of a private capture. Its arm and pass snapshots are
//! independent of the guest's display recorder in `d3d_diag`.

use agent::compare::Rgba;
use agent::render::private_graph::PrivateGraph;
use agent_proto::{CaptureLighting, frame_graph::PrivateFrameStats};
use d3d::Lighting;
use d3d::backend::DrawOutcome;
use d3d::scene_sink::{ReplayFrame, ReplayOp};
use frame_scene::{FrameScene, TargetRef};
use native_render::frame_graph::{CapturedFrame, PassObserver, PassRow, ReadColour};

pub struct PrivateFrame<'a> {
    replay: &'a ReplayFrame,
    next_op: usize,
    graph: PrivateGraph,
}

impl<'a> PrivateFrame<'a> {
    pub fn begin(scene: &FrameScene, replay: &'a ReplayFrame, present: u64, lighting: Lighting) -> Self {
        let graph = PrivateGraph::begin(PrivateFrameStats {
            map_epoch: scene.id.map_epoch,
            frame: scene.id.frame,
            tick: scene.id.tick,
            present,
            lighting: match lighting {
                Lighting::Retail => CaptureLighting::Retail,
                Lighting::E3 => CaptureLighting::E3,
            },
            // The successful readback supplies the hash and backend deltas.
            scene_hash: String::new(),
            passes: 0,
            draws: 0,
            clears: 0,
            copies: 0,
            render_passes: 0,
            backend_draws: 0,
            submits: 0,
        });
        Self { replay, next_op: 0, graph }
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
        self.graph.complete(output.scene_hash, output.report.render_passes, output.report.draws, output.report.submits);
    }
}

impl PassObserver for PrivateFrame<'_> {
    fn pass_end(&mut self, index: usize, row: &PassRow, read_colour: &mut dyn FnMut() -> ReadColour) {
        self.between(row.ops.start);
        let depth = (row.depth != TargetRef::None).then_some(row.depth);
        self.graph.pass(|| format!("{:?}", row.colour), || depth.map(|s| format!("{s:?}")));
        for (at, op) in &self.replay.ops[row.ops.clone()] {
            match op {
                ReplayOp::Draw { draw, .. } => {
                    self.graph.slot(at.view, at.slot.name());
                    self.graph.draw(|| crate::d3d_diag::probe_draw(row.colour, depth, draw, &Ok(DrawOutcome::Drawn)));
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
            Ok(Rgba { width, height, pixels })
        });
    }
}
