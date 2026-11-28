//! The harness end to end on the toy guest (`lockstep::synthetic`): the
//! per-tick hash through the tick observer, input recorded and replayed at
//! the guest's reads (`xapi::tap`), and the comparison. Equal inputs give
//! equal traces; one changed read is caught on the tick that consumed it.

use lockstep::synthetic::{self, Toy};
use lockstep::{Kind, Recorder, Trace, Verdict, compare, install_tick_observer};
use xapi::tap::{InputLog, InputTap, Recorder as InputRecorder, Replayer};

const FRAMES: u32 = 90;
const HANDLE: u32 = 0x5800_0000;
const CROUCH: u16 = 0x40;

/// The tick whose landing is the toy's super bounce under [`walk`].
const BOUNCE: usize = 76;

/// Walk right, jump every 24 frames (only the ones on the ground take),
/// crouch from frame 60 to 80: the second landing, at tick 76, is on a seam.
fn walk(frame: u32) -> [u8; 22] {
    let jump = if frame % 24 < 3 { 255 } else { 0 };
    let crouch = if (60..80).contains(&frame) { CROUCH } else { 0 };
    synthetic::packet(frame, crouch, jump, 20_000)
}

/// A different live pad: nothing pressed at all.
fn idle(frame: u32) -> [u8; 22] {
    synthetic::packet(frame, 0, 0, 0)
}

struct Run {
    trace: Trace,
    peak: i32,
}

fn run(mut tap: Box<dyn InputTap>, live: fn(u32) -> [u8; 22]) -> Run {
    let mut toy = Toy::new();
    let recorder = Recorder::new(synthetic::HOOK.game_time);
    install_tick_observer(
        &toy.runtime,
        synthetic::HOOK,
        std::sync::Arc::clone(&recorder),
    )
    .unwrap();
    let mut peak = 0;
    for f in 0..FRAMES {
        let (mut status, mut packet) = (0, live(f));
        tap.read(HANDLE, &mut status, &mut packet);
        toy.set_input(&packet);
        toy.frame();
        peak = peak.max(toy.height());
        recorder.record_presents(u64::from(f) + 1, &toy.runtime.memory(), Some(toy.clock()));
    }
    Run {
        trace: recorder.trace(),
        peak,
    }
}

fn record(live: fn(u32) -> [u8; 22]) -> (Run, InputLog) {
    let (tap, log) = InputRecorder::new();
    let r = run(Box::new(tap), live);
    let log = log.lock().unwrap().clone();
    (r, log)
}

#[test]
fn the_same_input_gives_the_same_trace() {
    let (a, log) = record(walk);
    let (b, _) = record(walk);
    assert_eq!(a.trace.ticks(), FRAMES as usize);
    assert_eq!(a.trace.records.len(), 2 * FRAMES as usize);
    assert!(
        compare(&a.trace, &b.trace).identical(),
        "{}",
        compare(&a.trace, &b.trace)
    );
    // The records carry the guest's own tick count and the logical clock.
    let ticks: Vec<_> = a
        .trace
        .records
        .iter()
        .filter(|r| r.kind == Kind::Tick)
        .collect();
    assert_eq!(ticks[5].game_tick, Some(6));
    assert_eq!(ticks[5].clock, Some(6 * synthetic::FRAME_TICKS));
    assert_eq!(log.reads.len(), FRAMES as usize);
    // The toy's bounce happened: higher than a plain jump can go.
    assert!(a.peak > 20_000, "peak {}", a.peak);
}

#[test]
fn a_replay_reproduces_the_recording_whatever_the_live_pad_does() {
    let (a, log) = record(walk);
    let (replayer, stats) = Replayer::new(log);
    let b = run(Box::new(replayer), idle);
    assert!(
        compare(&a.trace, &b.trace).identical(),
        "{}",
        compare(&a.trace, &b.trace)
    );
    assert_eq!(stats.lock().unwrap().replayed, u64::from(FRAMES));
    // And the trace survives its text form.
    assert_eq!(Trace::parse(&a.trace.to_text()).unwrap(), a.trace);
    // Without the replay the idle pad goes elsewhere at once.
    let (c, _) = record(idle);
    assert!(matches!(
        compare(&a.trace, &c.trace),
        Verdict::Diverged { index: 0, .. }
    ));
}

#[test]
fn one_changed_read_is_caught_on_the_tick_that_used_it() {
    let (a, log) = record(walk);
    // Let go of crouch for the one read the landing tick sees.
    let k = BOUNCE;
    let mut changed = log.clone();
    changed.reads[k].packet[4] &= !(CROUCH as u8);
    let (replayer, _) = Replayer::new(changed);
    let b = run(Box::new(replayer), walk);
    let v = compare(&a.trace, &b.trace);
    match &v {
        Verdict::Diverged {
            index,
            a: ra,
            chunks,
            ..
        } => {
            // Ticks and presents alternate; read k feeds tick k.
            assert_eq!(*index, 2 * k);
            assert_eq!((ra.kind, ra.seq), (Kind::Tick, k as u64));
            assert_eq!(chunks.len(), 1, "one page of toy data: {v}");
        }
        v => panic!("not caught: {v}"),
    }
    assert!(
        a.peak > 20_000 && b.peak < 10_000,
        "the bounce is lost: {} then {}",
        a.peak,
        b.peak
    );
    // A change to a read the guest ignores (the packet number) is still a
    // change to guest memory, and the harness says so.
    let mut renumbered = log;
    renumbered.reads[10].packet[0] ^= 0x80;
    let (replayer, _) = Replayer::new(renumbered);
    let c = run(Box::new(replayer), walk);
    assert!(matches!(
        compare(&a.trace, &c.trace),
        Verdict::Diverged { index: 20, .. }
    ));
}

#[test]
fn the_observer_refuses_a_tick_someone_else_wraps() {
    let toy = Toy::new();
    toy.runtime.edit_dispatch(|d| {
        let _ = d.register_override(synthetic::TICK, "someone else", |_, _, _| {
            Ok(runtime::TransferAbi::stop(0))
        });
    });
    let e = install_tick_observer(&toy.runtime, synthetic::HOOK, Recorder::new(None)).unwrap_err();
    assert!(e.contains("someone else"), "{e}");
    let bare = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
    assert!(install_tick_observer(&bare, synthetic::HOOK, Recorder::new(None)).is_err());
}

/// The tick collision, fixed: Live telemetry's sample used to make the
/// observer refuse the tick. Now both listen on the `frame` bridge and the
/// trace is the one the observer makes alone.
#[test]
fn the_hash_shares_the_tick_with_another_listener() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let alone = record(walk).0.trace;

    let mut toy = Toy::new();
    let recorder = Recorder::new(synthetic::HOOK.game_time);
    let samples = std::sync::Arc::new(AtomicU32::new(0));
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    r.add(&lockstep::TickObserver(std::sync::Arc::clone(&recorder)))
        .unwrap();
    let s = std::sync::Arc::clone(&samples);
    r.add(&hooks::from_fn("live-telemetry", move |b| {
        let s = std::sync::Arc::clone(&s);
        b.frame().on_tick("sample", move |_| {
            s.fetch_add(1, Ordering::Relaxed);
        });
        Ok(())
    }))
    .unwrap();
    let addrs = hooks::Remap::new(&[(&guest_layout::points::GAME_TICK, synthetic::TICK)]);
    r.build(&addrs).unwrap().install(&toy.runtime).unwrap();
    let (mut tap, _) = InputRecorder::new();
    for f in 0..FRAMES {
        let (mut status, mut packet) = (0, walk(f));
        tap.read(HANDLE, &mut status, &mut packet);
        toy.set_input(&packet);
        toy.frame();
        recorder.record_presents(u64::from(f) + 1, &toy.runtime.memory(), Some(toy.clock()));
    }
    assert_eq!(samples.load(Ordering::Relaxed), FRAMES);
    assert_eq!(
        compare(&alone, &recorder.trace()),
        Verdict::Identical {
            records: alone.records.len()
        }
    );
    assert_eq!(alone.ticks(), FRAMES as usize);
}
