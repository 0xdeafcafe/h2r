//! What the frame step hands the renderer (docs/graphics/native-renderer.md,
//! "How a frame flows"): a sealed [`Frame`] of the scene and its ordered
//! [`FrameOp`]s. Each slot's glue pushes its typed [`Stage`] with
//! [`frame_builder`]; the frame step seals the frame at the present and
//! publishes it to the render thread.

use std::sync::{Arc, Mutex};

use frame_scene::FrameScene;
use frame_types::{SlotAt, TargetRef};
use gfx::Clear;
use gfx::external::{ExternalAttachments, ExternalDraws, ExternalEncoded};
use gfx::present_camera::Present;

use crate::lighting::Lighting;
use crate::program::ViewInputs;

/// One present's work: the scene it draws from, the ops in the order the
/// frame step made them, and the present's camera metadata.
pub struct Frame {
    pub scene: Arc<FrameScene>,
    pub ops: Vec<FrameOp>,
    pub present: Present,
}

/// A step of the frame, in order. Targets are named by [`TargetRef`];
/// the frame graph's `Targets` owns the textures behind them.
pub enum FrameOp {
    /// The attachments the following stages draw into.
    Bind { colour: TargetRef, depth: TargetRef },
    Clear { colour: TargetRef, depth: TargetRef, clear: Clear },
    Copy { from: TargetRef, to: TargetRef },
    /// A typed stage at its slot, drawn on the bound attachments.
    Stage { slot: SlotAt, stage: Box<dyn Stage> },
}

impl std::fmt::Debug for FrameOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind { colour, depth } => f.debug_struct("Bind").field("colour", colour).field("depth", depth).finish(),
            Self::Clear { colour, depth, clear } => f
                .debug_struct("Clear")
                .field("colour", colour)
                .field("depth", depth)
                .field("clear", clear)
                .finish(),
            Self::Copy { from, to } => f.debug_struct("Copy").field("from", from).field("to", to).finish(),
            Self::Stage { slot, stage } => f.debug_struct("Stage").field("slot", slot).field("stage", &stage.name()).finish(),
        }
    }
}

/// A native stage the frame graph records on its attachments.
pub trait Stage: Send {
    /// The stage's name, for panics and the frame report.
    fn name(&self) -> &'static str;

    /// The one lighting mode whose frames draw this stage; `None` for
    /// every mode (docs/graphics/e3-lighting.md).
    fn lighting_only(&self) -> Option<Lighting> {
        None
    }

    /// The stage's work on `t` for `view`: packets appended to the open
    /// pass, or commands encoded on their own.
    fn record(&mut self, t: &ExternalAttachments, view: &ViewInputs) -> StageWork;
}

/// What a [`Stage`] records.
pub enum StageWork {
    Draws(ExternalDraws),
    Encoded(ExternalEncoded),
}

/// The open frame's ops, pushed by the frame step's glue in order.
pub struct FrameBuilder {
    ops: Mutex<Vec<FrameOp>>,
}

impl FrameBuilder {
    /// Appends `op` to the open frame.
    pub fn push(&self, op: FrameOp) {
        self.lock().push(op);
    }

    /// Ends the open frame: its ops so far, drawn from `scene` and shown
    /// with `present`. The next push starts the next frame.
    pub fn seal(&self, scene: Arc<FrameScene>, present: Present) -> Frame {
        let ops = std::mem::take(&mut *self.lock());
        Frame { scene, ops, present }
    }

    /// Drops the open frame's ops (a map load or a revert).
    pub fn discard(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<FrameOp>> {
        self.ops.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The process's one frame builder.
pub fn frame_builder() -> &'static FrameBuilder {
    static BUILDER: FrameBuilder = FrameBuilder { ops: Mutex::new(Vec::new()) };
    &BUILDER
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Named;
    impl Stage for Named {
        fn name(&self) -> &'static str {
            "named"
        }
        fn record(&mut self, _: &ExternalAttachments, _: &ViewInputs) -> StageWork {
            unimplemented!("test: record")
        }
    }

    #[test]
    fn seal_takes_the_ops_in_push_order_and_empties_the_builder() {
        let b = FrameBuilder { ops: Mutex::new(Vec::new()) };
        b.push(FrameOp::Bind { colour: TargetRef::SceneColour, depth: TargetRef::SceneDepth });
        b.push(FrameOp::Stage { slot: SlotAt::default(), stage: Box::new(Named) });
        let ops = std::mem::take(&mut *b.lock());
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], FrameOp::Bind { .. }));
        assert!(matches!(&ops[1], FrameOp::Stage { stage, .. } if stage.name() == "named"));
        assert!(b.lock().is_empty());
    }
}
