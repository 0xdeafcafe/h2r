//! `H2_FPS` changes nothing the guest can see (docs/game/high-framerate.md).
//!
//! The toy guest (`lockstep::synthetic`) runs 30 Hz frames with the per-tick
//! hash on, replaying one input recording, and publishes each frame through
//! the shell's triple buffer. A presenter on a fake display clock takes and
//! redraws frames at each setting's rate, exactly as the window does: on
//! guest presents at 30, on its own schedule above. The guest's trace must
//! be the same at every setting, while the presenter's redraw count shows
//! the setting did take effect.

use std::sync::Arc;
use std::time::Duration;

use app::fps::{DisplayClock, DisplaySchedule, FakeDisplayClock, FpsTarget};
use app::mailbox::TripleBuffer;
use lockstep::synthetic::{self, Toy};
use lockstep::{Recorder, Trace, compare, install_tick_observer};
use xapi::tap::{InputLog, InputTap, Recorder as InputRecorder, Replayer};

const FRAMES: u32 = 60;
/// One guest frame of display time.
const FRAME: Duration = Duration::from_nanos(33_333_333);
/// The fake display clock's step: how often the "event loop" turns.
const POLL: Duration = Duration::from_millis(1);

fn pad(frame: u32) -> [u8; 22] {
    synthetic::packet(
        frame,
        if frame % 20 < 10 { 0x40 } else { 0 },
        if frame.is_multiple_of(16) { 255 } else { 0 },
        12_000,
    )
}

/// What the presenter did.
#[derive(Debug, Default)]
struct Shown {
    redraws: u64,
    /// Guest frames that reached the screen.
    taken: u64,
}

fn play(target: FpsTarget, mut tap: Box<dyn InputTap>) -> (Trace, Shown) {
    let mut toy = Toy::new();
    let recorder = Recorder::new(synthetic::HOOK.game_time);
    install_tick_observer(&toy.runtime, synthetic::HOOK, Arc::clone(&recorder)).unwrap();
    let mut mailbox = TripleBuffer::new();
    let mut slots = [0u64; 3];
    let mut clock = FakeDisplayClock::new(POLL);
    let mut schedule = DisplaySchedule::new(target);
    let mut shown = Shown::default();
    let mut on_screen = None;
    let mut now = Duration::ZERO;
    for f in 0..FRAMES {
        // Guest: read input, tick, present.
        let (mut status, mut packet) = (0, pad(f));
        tap.read(0x5800_0000, &mut status, &mut packet);
        toy.set_input(&packet);
        toy.frame();
        recorder.record_presents(u64::from(f) + 1, &toy.runtime.memory(), Some(toy.clock()));
        slots[mailbox.back()] = u64::from(f);
        let guest_presented = !mailbox.publish();
        // Window: event-loop turns until the next guest frame is due.
        let end = FRAME * (f + 1);
        let mut woken = guest_presented;
        while now < end {
            now = clock.now();
            if woken || schedule.due(now) {
                woken = false;
                if let Some(slot) = mailbox.take() {
                    on_screen = Some(slots[slot]);
                    shown.taken += 1;
                }
                assert!(on_screen.is_some());
                shown.redraws += 1;
            }
        }
    }
    (recorder.trace(), shown)
}

fn recording() -> InputLog {
    let (tap, log) = InputRecorder::new();
    let _ = play(FpsTarget::Thirty, Box::new(tap));
    log.lock().unwrap().clone()
}

#[test]
fn every_fps_setting_leaves_the_guest_trace_identical() {
    let log = recording();
    let mut base = None;
    for target in FpsTarget::ALL {
        let (replayer, _) = Replayer::new(log.clone());
        let (trace, shown) = play(target, Box::new(replayer));
        assert_eq!(trace.ticks(), FRAMES as usize);
        assert_eq!(
            shown.taken,
            u64::from(FRAMES),
            "{target}: every guest frame reaches the screen"
        );
        let per_frame = shown.redraws as f64 / f64::from(FRAMES);
        match target {
            FpsTarget::Thirty => assert_eq!(
                shown.redraws,
                u64::from(FRAMES),
                "30: one redraw per present"
            ),
            FpsTarget::Sixty => assert!((2.0..3.1).contains(&per_frame), "{target}: {per_frame}"),
            FpsTarget::OneTwenty => {
                assert!((4.0..5.1).contains(&per_frame), "{target}: {per_frame}")
            }
            FpsTarget::Uncapped => assert!(per_frame > 30.0, "{target}: {per_frame}"),
        }
        match &base {
            None => base = Some(trace),
            Some(b) => {
                let v = compare(b, &trace);
                assert!(v.identical(), "{target}: {v}");
            }
        }
    }
}

#[test]
fn the_check_would_see_a_guest_that_followed_the_display() {
    // A guest that (wrongly) took one extra tick per extra redraw would
    // diverge: the harness is not blind to a presenter-to-guest leak.
    let log = recording();
    let (replayer, _) = Replayer::new(log.clone());
    let (a, _) = play(FpsTarget::Thirty, Box::new(replayer));
    let mut toy = Toy::new();
    let recorder = Recorder::new(synthetic::HOOK.game_time);
    install_tick_observer(&toy.runtime, synthetic::HOOK, Arc::clone(&recorder)).unwrap();
    let (mut replayer, _) = Replayer::new(log);
    for f in 0..FRAMES {
        let (mut status, mut packet) = (0, pad(f));
        replayer.read(0x5800_0000, &mut status, &mut packet);
        toy.set_input(&packet);
        toy.frame();
        if f == 30 {
            toy.frame();
        }
        recorder.record_presents(u64::from(f) + 1, &toy.runtime.memory(), Some(toy.clock()));
    }
    assert!(!compare(&a, &recorder.trace()).identical());
}
