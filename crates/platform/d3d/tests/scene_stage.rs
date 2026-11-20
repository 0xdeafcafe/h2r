//! Scene stages (docs/graphics/framescene.md, slice 2): a typed stage is
//! one command in the render thread's queue. It records between the D3D
//! draws around it, its preparation runs on the render thread and not on
//! the game thread, and without a render thread (or while the backend
//! replays) none is taken, so the stages keep reserving on the game
//! thread. Synthetic device over the recording backend; no GPU.
#![cfg(feature = "gpu")]

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use d3d::{
    Device, ExternalPass, ExternalPlan, InterpBackend, Recorded, RecordingBackend, SparseMemory, ThreadedBackend,
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
fn a_scene_stage_records_between_the_draws_around_it_and_prepares_on_the_render_thread() {
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
    assert_eq!(order, ["draw", "Objects 3 d3d-render", "draw"]);
    let threads = seen.lock().unwrap().clone();
    assert!(!threads.is_empty());
    assert!(
        threads.iter().all(|t| t.as_deref() == Some("d3d-render")),
        "the stage's preparation ran on {threads:?}, not only the render thread"
    );
    let drawn: Vec<u32> = calls
        .iter()
        .filter_map(|c| match c {
            Recorded::Draw { color, .. } => Some(*color),
            _ => None,
        })
        .collect();
    let staged = calls.iter().find_map(|c| match c {
        Recorded::SceneStage { color, .. } => Some(*color),
        _ => None,
    });
    assert_eq!(staged, drawn.first().copied(), "into the target the draws went to");
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
fn a_replaying_backend_takes_no_scene_stage() {
    let replaying = InterpBackend::new(recorder(), || 60);
    let (d, _) = create(ThreadedBackend::new(replaying).unwrap(), UploadPolicy::Always);
    assert!(!d.queues_scene_stages(Stage::Gels));
    let off = InterpBackend::new(recorder(), || 30);
    let (d, _) = create(ThreadedBackend::new(off).unwrap(), UploadPolicy::Always);
    assert!(d.queues_scene_stages(Stage::Gels));
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
