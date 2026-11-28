//! Scene stages (docs/graphics/framescene.md, slice 2): a typed stage is
//! one command in the render thread's queue. It records between the D3D
//! draws around it, its preparation runs on the render thread and not on
//! the game thread, and without a render thread (or while the backend
//! replays) none is taken, so the stages keep reserving on the game
//! thread. Synthetic stages use the recording backend. The asset-upload
//! case uses a headless device without any game surfaces.
#![cfg(feature = "gpu")]

use std::sync::{Arc, Mutex};

use crate::common::*;
use d3d::{
    Device, ExternalPass, ExternalPlan, Recorded, RecordingBackend, SparseMemory, ThreadedBackend,
    UploadPolicy,
};
use timeline::native::Stage;

/// A stage's pass: the threads its preparation ran on. The recording
/// backend asks [`ExternalPass::items`] where a GPU backend prepares and
/// records.
struct Probe(Arc<Mutex<Vec<Option<String>>>>);

impl ExternalPass for Probe {
    fn items(&self, ranges: usize) -> usize {
        self.0.lock().unwrap().push(std::thread::current().name().map(str::to_owned));
        ranges
    }
}

fn recorder() -> RecordingBackend {
    let mut r = RecordingBackend::new();
    r.external_passes = true;
    r
}

fn plan(stage: Stage, ranges: usize) -> ExternalPlan {
    ExternalPlan { stage, view: 0, ranges, slots: Vec::new() }
}

fn quad<B: d3d::Backend>(d: &mut Device<B>, m: &mut SparseMemory, color: u32) {
    quad_vb(m, 0.0, 0.0, 32.0, 32.0, color);
    bind_quad(d, m);
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

#[test]
fn a_scene_stage_prepares_and_records_on_the_render_thread() {
    let (mut d, mut m) = create(ThreadedBackend::new(recorder()).unwrap(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    quad(&mut d, &mut m, 0xFF00_0000);
    assert!(d.begin_external_boundary(&mut m).unwrap());
    assert!(d.queues_scene_stages(Stage::Objects));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reservation = d
        .reserve_scene_stage(&mut m, plan(Stage::Objects, 3), Box::new(Probe(Arc::clone(&seen))))
        .unwrap()
        .expect("a render thread that records scene stages takes one");
    reservation.commit();
    assert_eq!(d.stats().external_passes, 1);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    d.backend.finish().unwrap();
    let calls = d.backend.with_inner(|r| r.calls.clone()).unwrap();
    let order: Vec<String> = calls
        .iter()
        .filter_map(|c| match c {
            Recorded::Draw { .. } => Some("draw".to_owned()),
            Recorded::SceneStage { stage, items, thread, .. } => {
                Some(format!("{stage:?} {items} {}", thread.as_deref().unwrap_or("?")))
            }
            _ => None,
        })
        .collect();
    assert_eq!(order, ["Objects 3 d3d-render"], "the guest's draws are drawn by nothing");
    let threads = seen.lock().unwrap().clone();
    assert!(!threads.is_empty());
    assert!(
        threads.iter().all(|t| t.as_deref() == Some("d3d-render")),
        "the stage's preparation ran on {threads:?}, not only the render thread"
    );
    let staged = calls.iter().find_map(|c| match c {
        Recorded::SceneStage { color, .. } => Some(*color),
        _ => None,
    });
    assert!(staged.is_some(), "the stage reached the backend");
    let events = d.take_events();
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn a_cancelled_reservation_queues_nothing() {
    let (mut d, mut m) = create(ThreadedBackend::new(recorder()).unwrap(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reservation = d
        .reserve_scene_stage(&mut m, plan(Stage::Water, 2), Box::new(Probe(Arc::clone(&seen))))
        .unwrap()
        .expect("taken");
    drop(reservation);
    quad(&mut d, &mut m, 0xFF00_00FF);
    d.backend.finish().unwrap();
    let calls = d.backend.with_inner(|r| r.calls.clone()).unwrap();
    assert!(!calls.iter().any(|c| matches!(c, Recorded::SceneStage { .. })));
    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(d.stats().external_passes, 0);
}

#[test]
fn without_a_render_thread_no_scene_stage_is_taken() {
    let (mut d, mut m) = create(recorder(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    assert!(!d.queues_scene_stages(Stage::Fog));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let r = d.reserve_scene_stage(&mut m, plan(Stage::Fog, 1), Box::new(Probe(Arc::clone(&seen)))).unwrap();
    assert!(r.is_none());
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn a_stage_that_dropped_one_stands_down_and_the_others_do_not() {
    let (mut d, mut m) = create(ThreadedBackend::new(recorder()).unwrap(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    assert!(d.queues_scene_stages(Stage::Decals));
    d3d::scene_stage::dropped(Stage::Decals, 4, "a synthetic drop");
    assert!(d3d::scene_stage::stood_down(Stage::Decals));
    assert!(!d.queues_scene_stages(Stage::Decals));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let r = d.reserve_scene_stage(&mut m, plan(Stage::Decals, 4), Box::new(Probe(Arc::clone(&seen)))).unwrap();
    assert!(r.is_none(), "it reserves on the game thread again");
    assert!(d.queues_scene_stages(Stage::Hud));
}

#[test]
fn a_stage_of_no_ranges_is_not_taken() {
    let (mut d, mut m) = create(ThreadedBackend::new(recorder()).unwrap(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let r = d.reserve_scene_stage(&mut m, plan(Stage::Particles, 0), Box::new(Probe(seen))).unwrap();
    assert!(r.is_none());
}

#[test]
fn replay_filters_one_shot_stages() {
    use d3d::scene_sink::{ReplayOp, SceneSink};
    use frame_scene::{Slot, SlotAt};

    struct Retained;

    impl ExternalPass for Retained {
        fn replays_scene(&self) -> bool { true }
    }

    let captured = Arc::new(Mutex::new(None));
    let out = Arc::clone(&captured);
    let worker = ThreadedBackend::new(recorder()).unwrap();
    let sink = SceneSink::new(worker, || Some(SlotAt { slot: Slot::Opaque, view: 0 }))
        .with_replay(move |frame| *out.lock().unwrap() = Some(frame));
    let (mut d, mut m) = create(sink, UploadPolicy::Always);
    set_states(&mut d, &mut m);
    let seen = Arc::new(Mutex::new(Vec::new()));
    assert!(d.reserve_scene_stage(&mut m, plan(Stage::World, 1), Box::new(Probe(seen.clone()))).unwrap().is_none());
    quad(&mut d, &mut m, 0xff00_ff00);

    d.reserve_scene_stage(&mut m, plan(Stage::World, 1), Box::new(Retained)).unwrap().unwrap().commit();
    d.swap(&mut m, 0).unwrap();
    let guard = captured.lock().unwrap();
    let frame = guard.as_ref().expect("the sink captured the swap");
    let order: Vec<_> = frame.ops.iter().filter_map(|(_, op)| match op {
        ReplayOp::Draw { .. } => Some("guest"),
        ReplayOp::Stage(_) => Some("stage"),
        _ => None,
    }).collect();
    assert_eq!(order, ["stage"], "the guest's draw is drawn by nothing");
    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(d.stats().external_passes, 1);
}


#[test]
fn a_replaying_sink_keeps_an_immediate_stage_it_refuses_untyped() {
    use d3d::scene_sink::{ReplayOp, SceneSink};
    use frame_scene::{Slot, SlotAt};

    struct Retained;

    impl ExternalPass for Retained {
        fn replays_scene(&self) -> bool { true }
    }

    let captured = Arc::new(Mutex::new(None));
    let out = Arc::clone(&captured);
    let worker = ThreadedBackend::new(recorder()).unwrap();
    let sink = SceneSink::new(worker, || Some(SlotAt { slot: Slot::Opaque, view: 0 }))
        .with_replay(move |frame| *out.lock().unwrap() = Some(frame));
    let (mut d, mut m) = create(sink, UploadPolicy::Always);
    set_states(&mut d, &mut m);
    quad(&mut d, &mut m, 0xff00_ff00);
    assert!(!d.external_pass_now(&mut m, Box::new(Retained)).unwrap(), "the recorder cannot retain an untyped pass");
    assert!(d.external_stage_now(&mut m, None, plan(Stage::Fog, 1), Box::new(Retained)).unwrap());
    assert_eq!(d.stats().external_passes, 1);
    d.swap(&mut m, 0).unwrap();
    let guard = captured.lock().unwrap();
    let frame = guard.as_ref().expect("the sink captured the swap");
    let stages: Vec<_> = frame.ops.iter().filter_map(|(_, op)| match op {
        ReplayOp::Stage(stage) => Some(stage.plan.stage),
        _ => None,
    }).collect();
    assert_eq!(stages, [Stage::Fog], "the immediate pass is kept typed");
}

#[test]
fn scene_asset_upload_runs_once_on_the_display_worker_without_a_frame_or_attachments() {
    use d3d::Backend;

    let Ok(ctx) = gfx::GfxContext::headless() else { return };
    let worker = ThreadedBackend::new(d3d::GfxBackend::new(ctx)).unwrap()
        .with_frame_owner(d3d::threaded::owner::FrameOwner {
            select: Box::new(|_, _| None), events: std::sync::Arc::new(()),
        });
    let mut sink = d3d::SceneSink::new(worker, || None).with_replay(|_| panic!("unexpected present"));
    let (send, recv) = std::sync::mpsc::channel();
    sink.upload_scene_assets(Box::new(move |device, _| {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None, size: 32, usage: wgpu::BufferUsages::VERTEX, mapped_at_creation: false,
        });
        send.send((buffer.size(), std::thread::current().name().map(str::to_owned))).unwrap();
    }));
    sink.inner_mut().with_inner(|gpu| assert!(gpu.last_presented().is_none())).unwrap();
    assert_eq!(recv.recv().unwrap(), (32, Some("d3d-render".to_string())));
}

#[test]
fn flare_counters_follow_committed_draws_only() {
    let read = |name: &str| {
        d3d::scene_stage::counters().into_iter().find(|(k, _)| k == name).map(|(_, v)| v).unwrap()
    };
    let (display, reference) = (read("scene_submitted_flares_display_draws"), read("scene_submitted_flares_reference_draws"));
    // Queueing a flare stage, or committing another stage, counts no flare draw.
    d3d::scene_stage::queued(Stage::Flares);
    d3d::scene_stage::submitted(Stage::Fog, true, 7);
    assert_eq!(read("scene_submitted_flares_display_draws"), display);
    d3d::scene_stage::submitted(Stage::Flares, true, 3);
    d3d::scene_stage::submitted(Stage::Flares, false, 5);
    assert_eq!(read("scene_submitted_flares_display_draws"), display + 3);
    assert_eq!(read("scene_submitted_flares_reference_draws"), reference + 5);
    assert!(d3d::scene_stage::counters().iter().all(|(k, _)| !k.starts_with("scene_stages_flares") && !k.starts_with("scene_dropped_flares")));
}
