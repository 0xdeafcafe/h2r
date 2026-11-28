//! A swallowed post call committed as a carried scene stage
//! (docs/graphics/framescene.md, slice 5): under a sink that keeps the
//! frame for its replay, a pass that records again commits once, on every
//! live copy, and the sink retains it as a stage in place of the guest's
//! packets. A pass that cannot record again, or a frame the guest
//! reference asked for, keeps the guest's packets. Synthetic post calls
//! over a headless GPU; skipped without one.
#![cfg(feature = "gpu")]

use std::sync::{Arc, Mutex};

use crate::common::*;
use d3d::scene_sink::{ReplayFrame, ReplayOp};
use d3d::{
    Backend, Device, ExternalPass, ExternalTarget, GfxBackend, GuestMemory, ScaledBackend, SceneSink,
    SparseMemory, Swallow, SwallowCall, SwallowPass, TargetCopy, UploadPolicy,
};
use frame_scene::{Slot, SlotAt};
use gfx::vertex::PrimitiveType;
use timeline::native::Stage;

const TEX_A: u32 = MISC + 0x4000;
const LEVEL_A: u32 = MISC + 0x4200;
const DATA_A: u32 = 0x0030_0000;
const LEVEL_BYTES: u32 = 32 * 32 * 4;
const CALL: SwallowCall = SwallowCall {
    entry: 0x0002_14f0,
    esp: STACK_TOP - 0x100,
    ret: 0x0001_39ba,
};

/// A post call's slot: `render_screen_effects` runs in the transparents.
fn post() -> Option<SlotAt> {
    Some(SlotAt { slot: Slot::Transparents, view: 0 })
}

fn context() -> Option<gfx::GfxContext> {
    let ctx = gfx::GfxContext::headless().ok()?;
    ctx.caps.depth32_stencil8.then_some(ctx)
}

fn share(ctx: &gfx::GfxContext) -> gfx::GfxContext {
    gfx::GfxContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        caps: ctx.caps,
    }
}

type Frames = Arc<Mutex<Vec<ReplayFrame>>>;

/// `inner` under a sink that keeps every frame for its replay.
fn replaying<B: Backend>(inner: B, guest_reference: bool) -> (SceneSink<B>, Frames) {
    let frames = Frames::default();
    let out = Arc::clone(&frames);
    let sink = SceneSink::new(inner, post)
        .with_replay(move |frame| out.lock().unwrap().push(frame))
        .with_guest_reference_request(move || guest_reference);
    (sink, frames)
}

fn setup<B: Backend>(backend: B) -> (Device<B>, SparseMemory, u32) {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    set_states(&mut d, &mut m);
    bind_quad(&mut d, &mut m);
    m.write_u32(CALL.esp, CALL.ret).unwrap();

    m.map(0x8000_0000 | DATA_A, LEVEL_BYTES);
    m.write(0x8000_0000 | DATA_A, &vec![0x30; LEVEL_BYTES as usize]).unwrap();
    let format = (2 << 4) | (0x12 << 8) | (1 << 16);
    let size = 31 | (31 << 12) | (1 << 24);
    let texture = [1 | (4 << 16) | 0x0100_0000, DATA_A, 0, format, size];
    let level = [1 | (5 << 16) | 0x0100_0000, DATA_A, 0, format, size, TEX_A];
    for (header, words) in [(TEX_A, &texture[..]), (LEVEL_A, &level[..])] {
        for (i, word) in words.iter().enumerate() {
            m.write_u32(header + 4 * i as u32, *word).unwrap();
        }
    }
    let back = d.bound_target()[0];
    (d, m, back)
}

fn quad(m: &mut SparseMemory, i: u32) {
    inline_quad(m, i as f32 * 12.0, 0.0, i as f32 * 12.0 + 8.0, 8.0, [50 + i as u8, 180, 72, 255]);
}

/// The post call's draws: two quads into the intermediate (slot 1), then
/// one into the back buffer (slot 0), and its return.
fn post_call<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, back: u32) {
    d.set_render_target(m, LEVEL_A, 0).unwrap();
    quad(m, 0);
    quad(m, 1);
    d.set_render_target(m, back, 0).unwrap();
    quad(m, 2);
    assert_eq!(d.drain_until_callback(m).unwrap(), None);
    d.finish_swallow(m, CALL.entry, CALL.esp).unwrap();
    assert!(!d.swallowing());
}

/// A post replacement of the call's three quads: it clears every slot
/// green on each copy it records, and notes the copy.
struct Green {
    replays: bool,
    copies: Arc<Mutex<Vec<TargetCopy>>>,
}

impl ExternalPass for Green {
    fn encode(&mut self, _: ExternalTarget<'_>) {}
    fn colour_only(&self) -> bool { true }
    fn replays_scene(&self) -> bool { self.replays }

    fn record_ranges(&mut self, t: ExternalTarget<'_>) -> usize {
        self.copies.lock().unwrap().push(t.copy);
        for slot in t.slots.iter().flatten() {
            drop(t.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("synthetic carried post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &slot.attachment,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            }));
        }
        3
    }
}

impl SwallowPass for Green {}

fn swallow(replays: bool, copies: &Arc<Mutex<Vec<TargetCopy>>>) -> Swallow {
    Swallow {
        call: Some(CALL),
        end_returns: Vec::new(),
        headers: vec![None, Some(LEVEL_A)],
        targets: vec![1, 1, 0],
        primitive: PrimitiveType::QuadList,
        vertices: 4,
        shapes: Vec::new(),
        drop: true,
        pass: Box::new(Green { replays, copies: Arc::clone(copies) }),
    }
}

/// The post call under `sink`, presented: the device's stats, the copies
/// the pass recorded, and the frame the sink kept.
fn run<B: Backend>(sink: SceneSink<B>, frames: &Frames, replays: bool) -> (d3d::Stats, Vec<TargetCopy>, ReplayFrame) {
    let (mut d, mut m, back) = setup(sink);
    let copies = Arc::default();
    assert!(d.swallow(&mut m, swallow(replays, &copies)).unwrap());
    post_call(&mut d, &mut m, back);
    let stats = d.stats();
    d.swap(&mut m, 0).unwrap();
    let frame = frames.lock().unwrap().pop().expect("the sink kept the presented frame");
    let copies = copies.lock().unwrap().clone();
    (stats, copies, frame)
}

fn stages(frame: &ReplayFrame) -> Vec<&d3d::SceneStage<frame_scene::TargetRef>> {
    frame.ops.iter().filter_map(|(_, op)| match op {
        ReplayOp::Stage(stage) => Some(stage),
        _ => None,
    }).collect()
}

fn draws(frame: &ReplayFrame) -> usize {
    frame.ops.iter().filter(|(_, op)| matches!(op, ReplayOp::Draw { .. })).count()
}

#[test]
fn a_replaying_post_pass_commits_once_and_is_retained_as_a_stage_with_no_packets() {
    let Some(ctx) = context() else { return };
    let (sink, frames) = replaying(GfxBackend::new(ctx), false);
    let (stats, copies, frame) = run(sink, &frames, true);
    assert_eq!(stats.swallow_commits, 1);
    assert_eq!(stats.swallow_draws_dropped, 3);
    assert_eq!(stats.swallow_draws_replayed, 0);
    assert_eq!(stats.swallow_refusals, 0);
    assert_eq!(copies, [TargetCopy::Reference], "recorded once, on the one live copy");
    let stages = stages(&frame);
    assert_eq!(stages.len(), 1, "{:?}", frame.ops);
    assert_eq!(stages[0].plan.stage, Stage::Post);
    assert_eq!(stages[0].plan.ranges, 3);
    assert_eq!(stages[0].surfaces.slots.iter().flatten().count(), 2, "the back buffer and the intermediate");
    assert_eq!(draws(&frame), 0, "no swallowed packet reaches the replay");
    assert!(!frame.refusals.contains(&"unrecorded_external_pass"), "{:?}", frame.refusals);
    assert!(!frame.refusals.contains(&"nonrepeatable_scene_stage"), "{:?}", frame.refusals);
}

#[test]
fn a_post_pass_that_cannot_record_again_replays_its_packets() {
    let Some(ctx) = context() else { return };
    let (sink, frames) = replaying(GfxBackend::new(ctx), false);
    let (stats, copies, frame) = run(sink, &frames, false);
    assert_eq!(stats.swallow_commits, 0);
    assert_eq!(stats.swallow_draws_replayed, 3);
    assert!(copies.is_empty(), "the sink refused it before any copy recorded");
    assert!(stages(&frame).is_empty());
    assert_eq!(draws(&frame), 0, "the guest's packets are named gaps, not draws");
}

#[test]
fn a_guest_reference_request_declines_the_carry() {
    let Some(ctx) = context() else { return };
    let (sink, frames) = replaying(GfxBackend::new(ctx), true);
    let (stats, copies, frame) = run(sink, &frames, true);
    assert_eq!(stats.swallow_commits, 0);
    assert_eq!(stats.swallow_draws_replayed, 3);
    assert!(copies.is_empty());
    assert!(stages(&frame).is_empty(), "no carried stage for the reference");
    assert_eq!(draws(&frame), 0, "the guest's packets are named gaps, not draws");
}

#[test]
fn a_carried_post_pass_records_complete_reference_and_display_copies() {
    let Some(ctx) = context() else { return };
    let scaled = ScaledBackend::with_single_copy(GfxBackend::new(share(&ctx)), GfxBackend::new(ctx), 2, false).unwrap();
    let (sink, frames) = replaying(scaled, false);
    let (stats, copies, frame) = run(sink, &frames, true);
    assert_eq!(copies, [TargetCopy::Reference, TargetCopy::Display]);
    assert_eq!(stats.swallow_commits, 1);
    assert_eq!(stats.swallow_draws_replayed, 0);
    assert_eq!(stages(&frame).len(), 1);
    assert_eq!(draws(&frame), 0);
}

#[test]
fn without_a_replay_a_carried_post_pass_still_commits_once() {
    let Some(ctx) = context() else { return };
    let (mut d, mut m, back) = setup(GfxBackend::new(ctx));
    let copies = Arc::default();
    assert!(d.swallow(&mut m, swallow(true, &copies)).unwrap());
    post_call(&mut d, &mut m, back);
    assert_eq!(d.stats().swallow_commits, 1);
    assert_eq!(d.stats().swallow_draws_replayed, 0);
    assert_eq!(*copies.lock().unwrap(), [TargetCopy::Reference]);
    d.flush_render_targets(&mut m).unwrap();
    let intermediate = m.read_vec(0x8000_0000 | DATA_A, LEVEL_BYTES as usize).unwrap();
    assert_eq!(&intermediate[..4], [0, 255, 0, 255], "the carried commit wrote the intermediate");
}

/// Behind a render thread that takes scene stages the carried post stage
/// is queued in order, as the world's are: the game thread does not wait
/// for that thread to drain, and the sink still keeps the one stage.
#[test]
fn behind_a_render_thread_a_carried_post_pass_is_queued_not_waited_on() {
    let Some(ctx) = context() else { return };
    let queued = || {
        d3d::scene_stage::counters().into_iter().find(|(n, _)| n == "scene_stages_post").map_or(0, |(_, v)| v)
    };
    let before = queued();
    let threaded = d3d::ThreadedBackend::new(GfxBackend::new(ctx)).unwrap();
    let (sink, frames) = replaying(threaded, false);
    let (stats, _, frame) = run(sink, &frames, true);
    assert_eq!(stats.swallow_commits, 1);
    assert_eq!(stats.swallow_draws_replayed, 0);
    assert!(queued() > before, "the stage went to the render thread's queue");
    assert_eq!(stages(&frame).len(), 1, "{:?}", frame.ops);
    assert_eq!(draws(&frame), 0);
}
