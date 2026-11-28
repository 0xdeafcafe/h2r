//! The frame graph on a GPU: a frame's ops in order on its targets.

use std::sync::{Arc, Mutex};

use frame_scene::{SceneBuilder, SceneId, Stamp};
use frame_types::{Slot, SlotAt, TargetRef};
use gfx::external::{ExternalAttachments, ExternalEncoded, Lighting};
use gfx::present_camera::Present;
use gfx::{Clear, GfxContext};
use native_render::program::ViewInputs;
use native_render::{Frame, FrameOp, Renderer, Stage, StageWork};

/// Records the attachments' size and lighting, and fills its colour from
/// a copy of the target it names.
struct Probe {
    seen: Arc<Mutex<Vec<((u32, u32), Lighting)>>>,
    only: Option<Lighting>,
}

impl Stage for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }
    fn lighting_only(&self) -> Option<Lighting> {
        self.only
    }
    fn record(&mut self, t: &ExternalAttachments, view: &ViewInputs) -> StageWork {
        assert_eq!(view.size, t.size);
        self.seen.lock().unwrap().push((t.size, t.lighting));
        StageWork::Encoded(ExternalEncoded { ranges: 1, draws: 0, passes: 0, commands: Box::new(|_| {}) })
    }
}

fn frame(ops: Vec<FrameOp>) -> Frame {
    let mut b = SceneBuilder::default();
    b.begin(SceneId::default());
    let scene = b.seal(Stamp::default()).expect("a scene");
    Frame { scene, ops, present: Present::Guest { image: None, reference: false } }
}

fn clear(rgba: [f32; 4]) -> FrameOp {
    FrameOp::Clear {
        colour: TargetRef::SceneColour,
        depth: TargetRef::SceneDepth,
        clear: Clear { color: Some(rgba), depth: Some(1.0), stencil: Some(0) },
    }
}

#[test]
fn a_frame_clears_and_runs_its_stages_at_the_capture_scale() {
    let Ok(ctx) = GfxContext::headless() else { return };
    let mut renderer = Renderer::new(ctx, (640, 480));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let probe = |only| Box::new(Probe { seen: Arc::clone(&seen), only });
    let at = SlotAt { slot: Slot::Outside, view: 0 };
    let mut f = frame(vec![
        FrameOp::Bind { colour: TargetRef::SceneColour, depth: TargetRef::SceneDepth },
        clear([1.0, 0.0, 0.0, 1.0]),
        FrameOp::Stage { slot: at, stage: probe(None) },
        FrameOp::Stage { slot: at, stage: probe(Some(Lighting::E3)) },
    ]);
    let captured = renderer.capture_frame(&mut f, (1280, 960));
    assert_eq!(captured.size, [1280, 960]);
    assert!(captured.rgba.chunks_exact(4).all(|p| p == [255, 0, 0, 255]), "the clear fills the scene");
    assert_eq!(captured.report.stages, 1, "the E3-only stage is not drawn under retail");
    assert_eq!(*seen.lock().unwrap(), [((1280, 960), Lighting::Retail)]);

    // The same frame draws again: its stages record each time.
    renderer.capture_frame(&mut f, (640, 480));
    assert_eq!(seen.lock().unwrap().last(), Some(&((640, 480), Lighting::Retail)));
}

#[test]
fn a_later_clear_wins_and_a_colour_only_clear_keeps_the_depth() {
    let Ok(ctx) = GfxContext::headless() else { return };
    let mut renderer = Renderer::new(ctx, (640, 480));
    let mut f = frame(vec![
        clear([1.0, 0.0, 0.0, 1.0]),
        FrameOp::Clear {
            colour: TargetRef::SceneColour,
            depth: TargetRef::None,
            clear: Clear { color: Some([0.0, 0.0, 1.0, 1.0]), ..Clear::NONE },
        },
    ]);
    let captured = renderer.capture_frame(&mut f, (640, 480));
    assert!(captured.rgba.chunks_exact(4).all(|p| p == [0, 0, 255, 255]));
}

#[test]
#[should_panic(expected = "targets: aux 0")]
fn a_target_with_no_home_panics_by_name() {
    let Ok(ctx) = GfxContext::headless() else { panic!("targets: aux 0 (no GPU)") };
    let mut renderer = Renderer::new(ctx, (640, 480));
    let mut f = frame(vec![FrameOp::Copy { from: TargetRef::SceneColour, to: TargetRef::Aux(0) }]);
    renderer.capture_frame(&mut f, (640, 480));
}

/// Records the sizes of the levels it samples and of its attachments.
struct Sampler {
    samples: Vec<TargetRef>,
    seen: Arc<Mutex<Vec<(Vec<Option<(u32, u32)>>, (u32, u32))>>>,
}

impl Stage for Sampler {
    fn name(&self) -> &'static str {
        "sampler"
    }
    fn samples(&self) -> Vec<TargetRef> {
        self.samples.clone()
    }
    fn record(&mut self, t: &ExternalAttachments, _: &ViewInputs) -> StageWork {
        let levels = t.sampled.iter().map(|l| l.as_ref().map(|l| l.size)).collect();
        self.seen.lock().unwrap().push((levels, t.size));
        StageWork::Encoded(ExternalEncoded { ranges: 1, draws: 0, passes: 0, commands: Box::new(|_| {}) })
    }
}

fn game(n: u8, size: (u32, u32)) -> TargetRef {
    TargetRef::Game(frame_types::GameTarget { role: frame_types::TargetRole::Game(n), size, depth: false })
}

/// A game target is its role's surface at its guest size times the
/// scale: copied into, drawn on with no depth bound, and handed to a
/// stage that samples it in its order.
#[test]
fn a_stage_samples_game_targets_in_its_order() {
    let Ok(ctx) = GfxContext::headless() else { return };
    let mut renderer = Renderer::new(ctx, (640, 480));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let at = SlotAt { slot: Slot::Outside, view: 0 };
    let sampler = |samples| Box::new(Sampler { samples, seen: Arc::clone(&seen) });
    let (refraction, reflection) = (game(0x16, (640, 480)), game(0x15, (512, 256)));
    let mut f = frame(vec![
        clear([0.0, 1.0, 0.0, 1.0]),
        FrameOp::Copy { from: TargetRef::SceneColour, to: refraction },
        FrameOp::Bind { colour: reflection, depth: TargetRef::None },
        FrameOp::Stage { slot: at, stage: sampler(Vec::new()) },
        FrameOp::Bind { colour: TargetRef::SceneColour, depth: TargetRef::SceneDepth },
        FrameOp::Stage { slot: at, stage: sampler(vec![refraction, TargetRef::None, reflection]) },
    ]);
    renderer.capture_frame(&mut f, (1280, 960));
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0], (Vec::new(), (1024, 512)), "the reflection drawn on at the scale");
    assert_eq!(seen[1], (vec![Some((1280, 960)), None, Some((1024, 512))], (1280, 960)));
}

#[test]
#[should_panic(expected = "frame graph: stage sampler samples")]
fn a_stage_that_samples_its_attachment_panics_by_name() {
    let Ok(ctx) = GfxContext::headless() else { panic!("frame graph: stage sampler samples (no GPU)") };
    let mut renderer = Renderer::new(ctx, (640, 480));
    let target = game(0x15, (512, 256));
    let stage = Box::new(Sampler { samples: vec![target], seen: Arc::default() });
    let mut f = frame(vec![
        FrameOp::Bind { colour: target, depth: TargetRef::None },
        FrameOp::Stage { slot: SlotAt { slot: Slot::Outside, view: 0 }, stage },
    ]);
    renderer.capture_frame(&mut f, (640, 480));
}
