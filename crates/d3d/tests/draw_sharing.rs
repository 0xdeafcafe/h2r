//! `Device::set_draw_sharing` (`H2_D3D_DRAW_SHARING`): draws that read the
//! same state arrays as the pushed snapshot share it, and a vertex program
//! the game switches back to gets its earlier microcode copy. Only which
//! allocations back a draw changes: the backend sees the same calls, the
//! judge the same verdicts, and the device the same stats and events.
mod common;
mod draw_frame;

use std::sync::Arc;

use d3d::backend::{Recorded, RecordingBackend, ResolvedDraw};
use d3d::shader::VertexProcessing;
use d3d::{Backend, Device, GuestMemory, SparseMemory, ThreadedBackend};
use draw_frame::{NullBackend, frame, setup};

const FRAMES: u32 = 3;
const DRAWS: u32 = 240;

fn run<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    for f in 0..FRAMES {
        frame(d, m, DRAWS, f);
    }
}

fn microcode(d: &ResolvedDraw) -> &d3d::shader::Microcode {
    match &d.shader.vertex {
        VertexProcessing::Program { microcode, .. } => microcode,
        other => panic!("a vertex program, not {other:?}"),
    }
}

/// Recorded calls (draws compared field by field), stats, events and the
/// guest's view of memory, with sharing `on`.
fn recorded(on: bool) -> (Vec<Recorded>, d3d::Stats, Vec<d3d::Event>, SparseMemory) {
    let (mut d, mut m) = setup(RecordingBackend::new(), on);
    run(&mut d, &mut m);
    let events = d.take_events();
    (std::mem::take(&mut d.backend.calls), d.stats(), events, m)
}

#[test]
fn sharing_gives_the_backend_the_same_calls() {
    let (calls_on, stats_on, events_on, mem_on) = recorded(true);
    let (calls_off, stats_off, events_off, mem_off) = recorded(false);
    let draws = calls_on
        .iter()
        .filter(|c| matches!(c, Recorded::Draw { .. }))
        .count();
    assert_eq!(draws, (FRAMES * DRAWS) as usize, "every draw drawn");
    assert_eq!(calls_on.len(), calls_off.len());
    for (i, (a, b)) in calls_on.iter().zip(&calls_off).enumerate() {
        assert_eq!(a, b, "backend call {i}");
    }
    assert_eq!(stats_on, stats_off);
    assert_eq!(events_on, events_off);
    // The guest's side: device words, push buffer, state arrays.
    for addr in (common::MISC..common::MISC + 0x8000).step_by(4) {
        assert_eq!(
            mem_on.read_u32(addr).ok(),
            mem_off.read_u32(addr).ok(),
            "{addr:#x}"
        );
    }
    let l = d3d::GuestLayout::HALO2;
    for addr in (l.device..l.device + 0x1000).step_by(4) {
        assert_eq!(
            mem_on.read_u32(addr).ok(),
            mem_off.read_u32(addr).ok(),
            "{addr:#x}"
        );
    }
}

#[test]
fn sharing_reuses_the_snapshot_and_earlier_microcode() {
    let shared = |on: bool| {
        let (mut d, mut m) = setup(RecordingBackend::new(), on);
        run(&mut d, &mut m);
        let draws = d.backend.draws();
        let same_raw = draws
            .windows(2)
            .filter(|w| Arc::ptr_eq(&w[0].raw, &w[1].raw))
            .count();
        let equal_raw = draws.windows(2).filter(|w| w[0].raw == w[1].raw).count();
        // Draws of the same program (by words) whose copies differ.
        let mut copies: Vec<&d3d::shader::Microcode> = Vec::new();
        for draw in &draws {
            let mc = microcode(draw);
            if !copies.iter().any(|c| c.same(mc)) {
                copies.push(mc);
            }
        }
        (same_raw, equal_raw, copies.len())
    };
    let (same_on, equal_on, copies_on) = shared(true);
    let (same_off, equal_off, copies_off) = shared(false);
    assert_eq!(equal_on, equal_off, "the same words either way");
    assert!(equal_on > 0, "some draws read unchanged arrays");
    // A setter between two draws may copy the snapshot (copy on write)
    // even when it stores the value already there; the draw after it then
    // shares that copy rather than the previous draw's.
    assert!(
        same_on > 0 && same_on <= equal_on,
        "{same_on} of {equal_on}"
    );
    assert_eq!(same_off, 0, "without sharing every draw has its own");
    assert_eq!(copies_on, 2, "one copy per program: A, B, A, ... share");
    assert!(copies_off > 2, "without sharing a switch back copies again");
}

/// Behind the render thread the inner backend gets the same calls in the
/// same order.
#[test]
fn sharing_gives_the_render_thread_the_same_calls() {
    let calls = |on: bool| {
        let (mut d, mut m) = setup(ThreadedBackend::new(RecordingBackend::new()).unwrap(), on);
        run(&mut d, &mut m);
        assert_eq!(d.backend.misses(), 0);
        d.backend
            .with_inner(|b| std::mem::take(&mut b.calls))
            .unwrap()
    };
    let (on, off) = (calls(true), calls(false));
    assert_eq!(on.len(), off.len());
    for (i, (a, b)) in on.iter().zip(&off).enumerate() {
        assert_eq!(a, b, "backend call {i}");
    }
}

/// The real shader judge (`Nv2aJudge`) on the guest's side gives the same
/// verdicts, and the render thread agrees with every one.
#[test]
fn sharing_gives_the_judge_the_same_verdicts() {
    let judged = |on: bool| {
        let (mut d, mut m) = setup(ThreadedBackend::new(NullBackend::judged()).unwrap(), on);
        run(&mut d, &mut m);
        let events = d.take_events();
        let stats = d.stats();
        let (draws, sink) = d.backend.with_inner(|b| (b.draws, b.sink)).unwrap();
        assert_eq!(d.backend.misses(), 0, "the render thread agrees");
        (events, stats, draws, sink)
    };
    let on = judged(true);
    assert_eq!(on.2, (FRAMES * DRAWS) as u64);
    assert_eq!(on, judged(false));
}
