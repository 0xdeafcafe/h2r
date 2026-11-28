//! `GetVisibilityTestResult` without waiting (docs/graphics/d3d.md, "Visibility
//! tests"): a recent test whose count is not back answers
//! `D3DERR_TESTINCOMPLETE`, as the hardware may, instead of waiting for the
//! render thread and the GPU.
//!
//! [`flares`] plays Halo's lens-flare pattern (`0x20bb0` and `0x207b0`):
//! each frame the reader asks for the tests ended two frames before and
//! keeps an object's last value on an incomplete answer, and the writer
//! asks for the test ended three frames before in the slot it is about to
//! reuse, and skips the object for the frame on an incomplete answer.
//! Synthetic inline quads through `RecordingBackend`; guest draws no native
//! stage takes reach no backend, so the counts are not checked, only the
//! answers' timing.
//!
//! `cargo bench -p d3d --bench visibility_wait` prints where the guest
//! waited with a lagging render thread, polling and waiting.

mod common;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use common::*;
use d3d::device::VISIBILITY_PENDING_SWAPS;
use d3d::hw::method as hw;
use d3d::{
    Backend, Device, GuestMemory, RecordingBackend, SparseMemory, ThreadedBackend, UploadPolicy,
};

/// Lens-flare objects; object `i` of set `s` uses index `s + 3 * i`.
const OBJECTS: u32 = 4;
const DEV: u32 = d3d::GuestLayout::HALO2.device;

/// What `0x20cec` pushes: `CLEAR_REPORT_VALUE` 1 and
/// `SET_ZPASS_PIXEL_COUNT_ENABLE` 1.
fn begin_inline(m: &mut SparseMemory) {
    let mut put = m.read_u32(DEV).unwrap();
    for w in [(2 << 18) | hw::CLEAR_REPORT_VALUE, 1, 1] {
        m.write_u32(put, w).unwrap();
        put += 4;
    }
    m.write_u32(DEV, put).unwrap();
}

/// Counted draws of object `obj`'s test ended in `frame`: 1 to 3, so the
/// counts change from frame to frame.
fn draws(frame: u32, obj: u32) -> u32 {
    (frame + obj) % 3 + 1
}

/// What one run of [`flares`] saw.
#[derive(Debug, Default)]
struct Run {
    /// Per frame, each object's value after the reader (`None`: never
    /// answered yet).
    frames: Vec<Vec<Option<u32>>>,
    /// Every answer: (frame the test ended, object, count).
    answers: Vec<(u32, u32, u32)>,
    /// Incomplete answers: (swaps since the test ended, object).
    incomplete: Vec<(u32, u32)>,
    /// Wall time the guest spent in `GetVisibilityTestResult`.
    get_time: Duration,
}

/// Halo's lens-flare pattern for `frames` frames. `before_reads` runs
/// before each frame's reads (to let a render thread catch up, or hold it).
fn flares<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    frames: u32,
    mut before_reads: impl FnMut(&mut Device<B>, u32),
) -> Run {
    let mut run = Run::default();
    let mut visible = vec![None; OBJECTS as usize];
    // The slot bits (0x5233F0): the frame and swap each slot's test ended in.
    let mut slots: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut swaps = 0u32;
    let get = |d: &mut Device<B>, m: &mut SparseMemory, run: &mut Run, idx: u32| {
        let t = Instant::now();
        let a = d.get_visibility_test_result(m, idx).unwrap();
        run.get_time += t.elapsed();
        a
    };
    for frame in 0..frames {
        before_reads(d, frame);
        // The reader (0x207b0): the set ended two frames before.
        let set = (frame + 1) % 3;
        for obj in 0..OBJECTS {
            let idx = set + 3 * obj;
            let Some(&(ended, swap)) = slots.get(&idx) else {
                continue;
            };
            match get(d, m, &mut run, idx) {
                Some(a) => {
                    run.answers.push((ended, obj, a.count));
                    visible[obj as usize] = Some(a.count);
                }
                None => run.incomplete.push((swaps - swap, obj)),
            }
        }
        // The writer (0x20bb0): this frame's set, reused.
        let set = frame % 3;
        for obj in 0..OBJECTS {
            let idx = set + 3 * obj;
            if let Some(&(ended, swap)) = slots.get(&idx) {
                match get(d, m, &mut run, idx) {
                    // The count is thrown away; the slot is free.
                    Some(a) => run.answers.push((ended, obj, a.count)),
                    None => {
                        run.incomplete.push((swaps - swap, obj));
                        continue;
                    }
                }
            }
            begin_inline(m);
            for _ in 0..draws(frame, obj) {
                inline_quad(m, 0.0, 0.0, 8.0, 8.0, [0, 255, 0, 255]);
            }
            d.end_visibility_test(m, idx).unwrap();
            slots.insert(idx, (frame, swaps));
        }
        // Uncounted work, so a lagging render thread has something to lag
        // behind.
        for _ in 0..24 {
            inline_quad(m, 0.0, 0.0, 64.0, 64.0, [255, 0, 0, 255]);
        }
        d.swap(m, 0).unwrap();
        swaps += 1;
        run.frames.push(visible.clone());
    }
    run
}

fn device<B: Backend>(backend: B, poll: bool) -> (Device<B>, SparseMemory) {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    set_states(&mut d, &mut m);
    d.set_vertex_shader(&mut m, gfx::vertex::fvf::XYZRHW | gfx::vertex::fvf::DIFFUSE)
        .unwrap();
    d.set_visibility_poll(poll);
    (d, m)
}

fn threaded(jitter: Option<u64>) -> ThreadedBackend<RecordingBackend> {
    let t = ThreadedBackend::new(RecordingBackend::new()).unwrap();
    match jitter {
        Some(seed) => t.with_jitter(seed),
        None => t,
    }
}

/// Incomplete answers come only within [`VISIBILITY_PENDING_SWAPS`].
fn check_answers(run: &Run) {
    for &(age, obj) in &run.incomplete {
        assert!(
            age <= VISIBILITY_PENDING_SWAPS,
            "object {obj}: incomplete {age} swaps after"
        );
    }
}

/// A backend that answers at once and a render thread that has caught up
/// before every read give the guest the same sequence, with no incomplete
/// answer: the same values the waiting device gave.
#[test]
fn a_caught_up_render_thread_answers_like_the_synchronous_backend() {
    use d3d::backend::Recorded;
    const FRAMES: u32 = 12;
    let (mut a, mut am) = device(RecordingBackend::new(), true);
    let direct = flares(&mut a, &mut am, FRAMES, |_, _| {});
    let (mut w, mut wm) = device(RecordingBackend::new(), false);
    let waited = flares(&mut w, &mut wm, FRAMES, |_, _| {});
    let (mut b, mut bm) = device(threaded(None), true);
    let queued = flares(&mut b, &mut bm, FRAMES, |d, _| d.backend.finish().unwrap());

    check_answers(&direct);
    assert!(direct.incomplete.is_empty() && queued.incomplete.is_empty());
    assert_eq!(direct.frames, waited.frames);
    assert_eq!(direct.frames, queued.frames);
    assert_eq!(direct.answers, queued.answers);
    assert_eq!(direct.answers.len(), waited.answers.len());
    // Every object has a value from frame 2 on.
    assert!(
        direct.frames[2].iter().all(Option::is_some),
        "{:?}",
        direct.frames
    );
    assert_eq!(a.stats(), b.stats());
    assert_eq!(a.stats().visibility_incomplete, 0);
    b.backend.finish().unwrap();
    // The same calls, but the render thread reads each count when it is
    // back (at the end of a batch), not when the guest asks, and also
    // those of the last tests, which the guest never asked for.
    let calls = b.backend.with_inner(|r| r.calls.clone()).unwrap();
    let split = |calls: &[Recorded]| -> (Vec<Recorded>, Vec<Recorded>) {
        calls
            .iter()
            .cloned()
            .partition(|c| !matches!(c, Recorded::ReadPixelCount { .. }))
    };
    let ((ca, ra), (cb, rb)) = (split(&a.backend.calls), split(&calls));
    assert_eq!(ca, cb);
    assert!(ra.iter().all(|r| rb.contains(r)), "{ra:?} {rb:?}");
    assert_eq!(rb.len(), ra.len() + 2 * OBJECTS as usize);
    assert_eq!(
        am.read_vec(DEV, 0x100).unwrap(),
        bm.read_vec(DEV, 0x100).unwrap()
    );
    let waits = b.backend.waits();
    assert_eq!(waits.pending_counts, 0);
    // One poll per test: the writer's second ask answers the kept count.
    assert_eq!(waits.landed_counts, ra.len() as u64);
    assert_eq!(
        waits.reads,
        FRAMES as u64 + 2,
        "the finishes, and the calls read here"
    );
}

/// The old behaviour (`Device::set_visibility_poll(false)`): with the render thread
/// lagging by random amounts, every call waits and the guest sees exactly
/// what the synchronous backend gives.
#[test]
fn waiting_never_answers_incomplete_whatever_the_render_thread_does() {
    const FRAMES: u32 = 9;
    let (mut a, mut am) = device(RecordingBackend::new(), false);
    let direct = flares(&mut a, &mut am, FRAMES, |_, _| {});
    for seed in [1, 0x9e37_79b9] {
        let (mut b, mut bm) = device(threaded(Some(seed)), false);
        let queued = flares(&mut b, &mut bm, FRAMES, |_, _| {});
        assert!(queued.incomplete.is_empty());
        assert_eq!(direct.frames, queued.frames, "seed {seed:#x}");
        assert_eq!(direct.answers, queued.answers, "seed {seed:#x}");
        assert_eq!(a.stats(), b.stats(), "seed {seed:#x}");
        assert_eq!(
            am.read_vec(DEV, 0x100).unwrap(),
            bm.read_vec(DEV, 0x100).unwrap()
        );
    }
}

/// Polling with the render thread lagging by random amounts: incomplete
/// answers come only for tests
/// ended at most two swaps before, and the reads of those never wait for
/// the render thread.
#[test]
fn polling_a_lagging_render_thread_answers_right_counts_or_incomplete() {
    const FRAMES: u32 = 9;
    for seed in [1, 0x9e37_79b9, 0xdead_beef] {
        let (mut b, mut bm) = device(threaded(Some(seed)), true);
        let run = flares(&mut b, &mut bm, FRAMES, |_, _| {});
        check_answers(&run);
        assert_eq!(b.stats().visibility_incomplete, run.incomplete.len() as u64);
        let w = b.backend.waits();
        assert_eq!(
            w.pending_counts,
            run.incomplete.len() as u64,
            "seed {seed:#x}"
        );
        assert_eq!(b.backend.misses(), 0);
    }
}

/// The original's incomplete path, through the ABI: `D3DERR_TESTINCOMPLETE`
/// in EAX, nothing written, and `D3D_SetFence` when the GPU time has not
/// moved since the End (only then). A test older than
/// [`VISIBILITY_PENDING_SWAPS`] swaps waits instead. Waiting mode never
/// answers incomplete.
#[test]
fn incomplete_writes_nothing_and_kicks_a_fence_once() {
    use d3d::backend::Recorded;
    use d3d::device::D3DERR_TESTINCOMPLETE;
    use d3d::layout::device_field as f;
    const GET: u32 = 0x3f8630;
    const OUT: u32 = MISC + 0x8000;
    const STAMP: u32 = MISC + 0x8010;
    let get = |d: &mut Device<RecordingBackend>, m: &mut SparseMemory, index: u32| {
        for a in [OUT, STAMP, STAMP + 4] {
            m.write_u32(a, 0xAAAA_AAAA).unwrap();
        }
        let r = d3d::abi::Regs {
            eax: index,
            edx: STAMP,
            ..d3d::abi::Regs::default()
        };
        let out = call(d, m, GET, r, &[OUT]).unwrap();
        let words = [OUT, STAMP, STAMP + 4].map(|a| m.read_u32(a).unwrap());
        (out.eax.unwrap(), words)
    };
    let (mut d, mut m) = device(RecordingBackend::new(), true);
    d.backend.pixel_counts_pending = true;
    begin_inline(&mut m);
    inline_quad(&mut m, 0.0, 0.0, 8.0, 8.0, [0, 255, 0, 255]);
    let time = m.read_u32(DEV + f::GPU_TIME).unwrap();
    d.end_visibility_test(&mut m, 5).unwrap();

    let (eax, words) = get(&mut d, &mut m, 5);
    assert_eq!(eax, D3DERR_TESTINCOMPLETE);
    assert_eq!(words, [0xAAAA_AAAA; 3], "incomplete writes nothing");
    assert_eq!(
        m.read_u32(DEV + f::GPU_TIME).unwrap(),
        time + 2,
        "the fence kick"
    );
    assert_eq!(d.gpu_completed(), time + 2);
    // The GPU time has moved: no second fence.
    assert_eq!(get(&mut d, &mut m, 5).0, D3DERR_TESTINCOMPLETE);
    assert_eq!(m.read_u32(DEV + f::GPU_TIME).unwrap(), time + 2);
    for _ in 0..VISIBILITY_PENDING_SWAPS {
        d.swap(&mut m, 0).unwrap();
        assert_eq!(get(&mut d, &mut m, 5).0, D3DERR_TESTINCOMPLETE);
    }
    assert_eq!(
        d.stats().visibility_incomplete,
        2 + VISIBILITY_PENDING_SWAPS as u64
    );
    // One swap more: the count is waited for, though not back.
    d.swap(&mut m, 0).unwrap();
    let (eax, words) = get(&mut d, &mut m, 5);
    assert_eq!(eax, 0);
    assert_eq!(words, [0, time, 0]);
    let reads = d
        .backend
        .calls
        .iter()
        .filter(|c| matches!(c, Recorded::ReadPixelCount { .. }));
    assert_eq!(reads.count(), 1);
    // Kept: asking again answers the same.
    assert_eq!(get(&mut d, &mut m, 5), (0, [0, time, 0]));

    // Waiting: answered at once, whatever the backend.
    d.set_visibility_poll(false);
    begin_inline(&mut m);
    d.end_visibility_test(&mut m, 6).unwrap();
    assert_eq!(get(&mut d, &mut m, 6).0, 0);
    assert_eq!(
        d.stats().visibility_incomplete,
        2 + VISIBILITY_PENDING_SWAPS as u64
    );
}

/// Guest wall time in `GetVisibilityTestResult` per frame with a render
/// thread that lags (0-300 us per command, about 10 ms a frame here),
/// waiting and polling.
pub fn bench() {
    let frames = std::env::var("H2_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30u32);
    println!("visibility results, {frames} frames of {OBJECTS} lens flares, render thread lagging");
    for (name, poll) in [
        ("wait (set_visibility_poll(false))", false),
        ("poll (default)", true),
    ] {
        let (mut d, mut m) = device(threaded(Some(0x9e37_79b9)), poll);
        let t = Instant::now();
        let run = flares(&mut d, &mut m, frames, |_, _| {});
        let total = t.elapsed();
        check_answers(&run);
        let w = d.backend.waits();
        let calls = run.answers.len() + run.incomplete.len();
        println!(
            "  {name:32} {:>8.3} ms/frame in GetVisibilityTestResult ({calls} calls, {} incomplete, {} waited reads), {:.1} ms/frame total",
            run.get_time.as_secs_f64() * 1e3 / frames as f64,
            run.incomplete.len(),
            w.reads,
            total.as_secs_f64() * 1e3 / frames as f64,
        );
    }
}

#[test]
#[ignore = "benchmark: cargo bench -p d3d --bench visibility_wait"]
fn visibility_wait() {
    bench();
}

#[test]
fn canonical_flare_tickets_and_mid_frame_reads_keep_their_original_proxy() {
    use d3d::scene_sink::{ReplayFrame, ReplayOp, SceneSink};
    use d3d::threaded::owner::{Candidate, FrameOwner, Submission, Outputs};
    use frame_scene::{Slot, SlotAt};
    use std::sync::{Arc, Mutex};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let scene = Arc::new(Mutex::new(None::<ReplayFrame>));
    let selected = Arc::clone(&scene);
    let accepted = Arc::new(AtomicUsize::new(0));
    let displayed = Arc::clone(&accepted);
    let owner = FrameOwner {
        select: Box::new(move |_, _| {
            let frame = Arc::new(selected.lock().unwrap().take()?);
            let played = Arc::clone(&frame);
            let displayed = Arc::clone(&displayed);
            Some(Submission::Native(Candidate {
                frame,
                before_original: None,
                eligible: Box::new(|_, _| true),
                draw: Box::new(move |gpu: &mut RecordingBackend, targets| {
                    displayed.fetch_add(1, Ordering::Relaxed);
                    let mut outputs = Outputs::default();
                    for (_, op) in &played.ops {
                        if let ReplayOp::EndFlareQuery(proxy) = op {
                            // Distinct from the zero samples drawn by the
                            // original empty queries; also distinct per slot.
                            gpu.pixel_count = 50 + *proxy;
                            let ticket = gpu.report_pixel_count()?.ok_or("missing synthetic pixel counter")?;
                            outputs.tickets.push((*proxy, ticket));
                        }
                    }
                    gpu.present(targets[&played.presented], 0)?;
                    Ok(outputs)
                }),
            }))
        }),
        events: std::sync::Arc::new(()),
    };
    let threaded = ThreadedBackend::new(RecordingBackend::new()).unwrap().with_frame_owner(owner);
    let mut sink = SceneSink::new(threaded, || Some(SlotAt { slot: Slot::Transparents, view: 1 }))
        .with_replay(move |frame| *scene.lock().unwrap() = Some(frame));
    let surface = sink.create_surface(gfx::D3DFormat::LinA8R8G8B8, 8, 8, false).unwrap();

    // A synchronous read before Swap needs the original queue. Its
    // commands must be issued exactly once; the owner must decline.
    sink.clear_pixel_count().unwrap();
    let first = sink.report_pixel_count().unwrap().unwrap();
    assert_eq!(sink.read_pixel_count(first), Ok(0));
    sink.present(surface, 0).unwrap();

    sink.clear_pixel_count().unwrap();
    let second = sink.report_pixel_count().unwrap().unwrap();
    sink.clear_pixel_count().unwrap();
    let third = sink.report_pixel_count().unwrap().unwrap();
    sink.present(surface, 0).unwrap();
    sink.inner_mut().finish().unwrap();
    assert_eq!(accepted.load(Ordering::Relaxed), 1);
    // Poll a later report first. Landing a different ticket must
    // neither block reception nor spend the earlier proxy.
    assert_eq!(sink.try_pixel_count(third), Ok(Some(50 + third)));
    assert_eq!(sink.read_pixel_count(second), Ok(50 + second));
}
