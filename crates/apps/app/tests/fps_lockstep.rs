//! `H2_FPS` changes nothing the guest can see (docs/game/high-framerate.md).
//!
//! The toy guest (`lockstep::synthetic`) runs 30 Hz frames with the per-tick
//! hash on, replaying one input recording, and publishes each frame through
//! the shell's triple buffer. A presenter on a fake display clock takes and
//! redraws frames at each setting's rate, exactly as the window does: on
//! every publish, and above 30 on its own schedule between publishes
//! (`DisplaySchedule::published`). The guest's trace must
//! be the same at every setting, while the presenter's redraw count shows
//! the setting did take effect.

use std::sync::Arc;
use std::time::Duration;

use app::fps::{DisplayClock, DisplaySchedule, FakeDisplayClock, FpsTarget};
use app::mailbox::TripleBuffer;
use app::{AppConfig, PaceAction, Pacer};
use lockstep::synthetic::{self, Toy};
use lockstep::{Recorder, Trace, compare, install_tick_observer};
use xapi::tap::{InputLog, InputTap, Recorder as InputRecorder, Replayer};

const FRAMES: u32 = 60;
/// A host fast enough to run the guest at twice real time without its pacer.
const HOST_WORK: Duration = Duration::from_nanos(16_666_667);
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
    /// Wall time when the pacer releases each completed guest frame.
    released: Vec<Duration>,
}

fn play(config: AppConfig, mut tap: Box<dyn InputTap>) -> (Trace, Shown) {
    let mut toy = Toy::new();
    let recorder = Recorder::new(synthetic::HOOK.game_time);
    install_tick_observer(&toy.runtime, synthetic::HOOK, Arc::clone(&recorder)).unwrap();
    let mut mailbox = TripleBuffer::new();
    let mut slots = [0u64; 3];
    let mut clock = FakeDisplayClock::new(POLL);
    let mut schedule = DisplaySchedule::new(config.fps);
    let mut pacer = Pacer::new(config.pacing);
    let mut wall = Duration::ZERO;
    assert_eq!(pacer.decide(toy.clock(), wall), PaceAction::Run);
    let mut shown = Shown::default();
    let mut on_screen = None;
    let mut now = clock.now();
    for f in 0..FRAMES {
        // Guest: read input, tick, present.
        let (mut status, mut packet) = (0, pad(f));
        tap.read(0x5800_0000, &mut status, &mut packet);
        toy.set_input(&packet);
        toy.frame();
        recorder.record_presents(u64::from(f) + 1, &toy.runtime.memory(), Some(toy.clock()));
        slots[mailbox.back()] = u64::from(f);
        let guest_presented = !mailbox.publish();
        // Only guest execution and the production pacer's sleep cost wall time.
        wall += HOST_WORK;
        if let PaceAction::Sleep(wait) = pacer.decide(toy.clock(), wall) {
            wall += wait;
        }
        shown.released.push(wall);
        // The window can keep presenting throughout that interval.
        let mut woken = guest_presented;
        while now < wall {
            if woken {
                schedule.published(now);
            }
            if woken || schedule.due(now) {
                woken = false;
                if let Some(slot) = mailbox.take() {
                    on_screen = Some(slots[slot]);
                    shown.taken += 1;
                }
                assert_eq!(on_screen, Some(u64::from(f)));
                shown.redraws += 1;
            }
            now = clock.now();
        }
    }
    (recorder.trace(), shown)
}

fn recording() -> InputLog {
    let (tap, log) = InputRecorder::new();
    let _ = play(AppConfig::default(), Box::new(tap));
    log.lock().unwrap().clone()
}

#[test]
fn every_fps_setting_leaves_the_guest_trace_identical() {
    let log = recording();
    let mut base = None;
    for target in FpsTarget::ALL {
        let (replayer, _) = Replayer::new(log.clone());
        let config = AppConfig {
            fps: target,
            ..AppConfig::default()
        };
        let (trace, shown) = play(config, Box::new(replayer));
        assert_eq!(trace.ticks(), FRAMES as usize);
        for second in 1..=2 {
            let end = Duration::from_secs(second);
            assert_eq!(
                shown.released.iter().filter(|&&t| t <= end).count(),
                30 * second as usize,
                "{target}: completed guest ticks by wall second {second}"
            );
        }
        // The toy rounds each two-vblank interval down to whole 100 ns ticks.
        for (i, &wall) in shown.released.iter().enumerate() {
            let guest = app::pacing::ticks_to_duration((i as u64 + 1) * synthetic::FRAME_TICKS);
            assert_eq!(wall, guest, "{target}: guest frame {}", i + 1);
        }
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
            FpsTarget::Sixty => assert_eq!(shown.redraws, 120),
            FpsTarget::OneTwenty => assert_eq!(shown.redraws, 240),
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
fn explicitly_unpaced_play_exposes_the_fast_hosts_tick_rate() {
    let config = AppConfig {
        fps: FpsTarget::Uncapped,
        pacing: app::PacePolicy::Unpaced,
        ..AppConfig::default()
    };
    let (tap, _) = InputRecorder::new();
    let (trace, shown) = play(config, Box::new(tap));
    assert_eq!(trace.ticks(), 60);
    assert_eq!(shown.released.last(), Some(&(60 * HOST_WORK)));
}

#[test]
fn the_check_would_see_a_guest_that_followed_the_display() {
    // A guest that (wrongly) took one extra tick per extra redraw would
    // diverge: the harness is not blind to a presenter-to-guest leak.
    let log = recording();
    let (replayer, _) = Replayer::new(log.clone());
    let (a, _) = play(AppConfig::default(), Box::new(replayer));
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
