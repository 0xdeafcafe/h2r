//! The one mixer consumer that takes DirectSound to the host's audio output.
//!
//! [`Hle::spawn_output`] starts a thread that owns an [`AudioSink`] (a
//! `audio::output::DeviceSink` in the app, anything else in tests), renders
//! the mixer into it and then completes whatever that render finished. It
//! replaces the discard renderers: one thread renders, so audio time runs
//! once, and the same thread reports completions, so stream packets keep
//! completing while the game sits in a loading loop and calls no DirectSound
//! entry at all.
//!
//! Order per wake, all on this thread:
//!
//! 1. render the frames owed (see [`Pacing`]) under the mixer lock alone and
//!    write them to the sink;
//! 2. collect completions exactly as `DirectSoundDoWork` does: each packet's
//!    completed size and status reach guest memory, in submission order,
//!    before its event is published;
//! 3. deliver the published events with no guard held. Stream callbacks are
//!    guest calls: they wait for the next override call on a guest thread,
//!    in order, as for any host-driven `Hle::do_work`.
//!
//! The sink is opened on the output thread (`DeviceSink` is not `Send` on
//! macOS) and dropped there.
use crate::register::{Hle, StopRecord};
use crate::state::{AudioClock, Consumer};
use audio::sink::{AudioSink, Pump as RingPump, SinkStats};
use runtime::Runtime;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How many frames the output thread renders each time it wakes.
pub enum Pacing {
    /// Keep `target_latency_ms` of audio queued in the sink: audio time
    /// follows the device's consumption, which is real time. The default for
    /// a real device. A sink nobody drains stops audio time once it is full.
    Device { target_latency_ms: u32 },
    /// Render what the guest's logical clock says has elapsed since the last
    /// wake: `now` returns logical time in 100 ns ticks (the kernel's
    /// `Clock::counter`), so packet completions, events and callbacks follow
    /// the time the game itself sees, deterministically. At most
    /// `max_catch_up` is rendered per wake. Rendered audio the sink has no
    /// room for is dropped and counted in [`OutputStats::dropped_frames`];
    /// if logical time runs slower than real time the device underruns
    /// instead. Audio time stops while logical time stands still: a game
    /// whose every thread waited on a stream event with no timeout would
    /// wait forever, so prefer `Device` pacing when a device is present.
    Logical {
        now: Box<dyn FnMut() -> u64 + Send>,
        max_catch_up: Duration,
    },
}

/// Settings for [`Hle::spawn_output`].
pub struct OutputConfig {
    /// How long the thread sleeps between wakes; also the worst-case delay
    /// before a finished packet is reported. Keep it well under the device
    /// pacing target.
    pub period: Duration,
    pub pacing: Pacing,
}

impl Default for OutputConfig {
    /// 5 ms wakes, device pacing at the audio crate's 40 ms target.
    fn default() -> Self {
        Self {
            period: Duration::from_millis(5),
            pacing: Pacing::Device {
                target_latency_ms: audio::sink::DEFAULT_TARGET_LATENCY_MS,
            },
        }
    }
}

/// Counters of a running [`Output`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputStats {
    /// Frames rendered from the mixer: the audio clock.
    pub rendered_frames: u64,
    /// Rendered frames the sink had no room for (logical pacing only).
    pub dropped_frames: u64,
    /// Wakes that completed packets and delivered events.
    pub wakes: u64,
    /// The sink's rate, which the mixer follows.
    pub sample_rate: u32,
    /// The sink's own counters (underruns, frames played).
    pub sink: SinkStats,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    rendered: AtomicU64,
    dropped: AtomicU64,
    wakes: AtomicU64,
    rate: AtomicU32,
    sink: Mutex<SinkStats>,
}

/// The output thread [`Hle::spawn_output`] started. Dropping it stops and
/// joins the thread, closes the sink and gives the mixer back (another
/// consumer may then claim it).
pub struct Output {
    shared: Arc<Shared>,
    join: Option<std::thread::JoinHandle<()>>,
    hle: Arc<Hle>,
}

impl Output {
    pub fn stats(&self) -> OutputStats {
        let s = &self.shared;
        OutputStats {
            rendered_frames: s.rendered.load(Ordering::Acquire),
            dropped_frames: s.dropped.load(Ordering::Acquire),
            wakes: s.wakes.load(Ordering::Acquire),
            sample_rate: s.rate.load(Ordering::Acquire),
            sink: *s.sink.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
    /// The thread has exited (dropped, or it hit a stop: see
    /// [`Hle::take_stops`], name `"Hle::output"`).
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(|j| j.is_finished())
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        self.hle.direct_sound().release_consumer(Consumer::Output);
    }
}

/// Largest single render, in frames.
const CHUNK: u64 = 1024;

impl Hle {
    /// Starts the output thread: `open` runs on it and returns the sink the
    /// mixer is rendered into (for the app,
    /// `audio::output::DeviceSink::open_default`). The mixer switches to the
    /// sink's sample rate, and follows it after a device change.
    ///
    /// This claims the mixer as [`Consumer::Output`]: it fails if
    /// `set_frames_per_do_work`, a render pump or `advance_audio` already
    /// consume it, and they fail while this runs. Nothing can check for a
    /// consumer outside this crate, so do not also hand the mixer to the
    /// app's `AudioOut` or `DiscardRenderer`, or to `audio::Pump`.
    pub fn spawn_output<S, F>(
        self: &Arc<Self>,
        runtime: Arc<Runtime>,
        open: F,
        config: OutputConfig,
    ) -> Result<Output, crate::state::ConsumerConflict>
    where
        S: AudioSink + 'static,
        F: FnOnce() -> S + Send + 'static,
    {
        self.direct_sound().claim_consumer(Consumer::Output)?;
        let shared = Arc::new(Shared::default());
        let hle = Arc::clone(self);
        let sh = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("dsound-output".into())
            .spawn(move || run(hle, runtime, open(), config, sh))
            .expect("spawning the dsound output thread");
        Ok(Output {
            shared,
            join: Some(join),
            hle: Arc::clone(self),
        })
    }
}

fn run<S: AudioSink>(
    hle: Arc<Hle>,
    runtime: Arc<Runtime>,
    mut sink: S,
    config: OutputConfig,
    shared: Arc<Shared>,
) {
    let mixer = Arc::clone(hle.direct_sound().mixer());
    let mut scratch = Vec::new();
    let mut pacing = match config.pacing {
        Pacing::Device { target_latency_ms } => Paced::Device(RingPump::new(target_latency_ms)),
        Pacing::Logical {
            mut now,
            max_catch_up,
        } => {
            let last = now();
            Paced::Logical {
                now,
                last,
                max_catch_up,
                clock: AudioClock::new(sink.sample_rate(), max_catch_up),
                rate: sink.sample_rate(),
            }
        }
    };
    while !shared.stop.load(Ordering::Acquire) {
        std::thread::sleep(config.period);
        let rendered = {
            let mut m = mixer.lock().unwrap_or_else(|e| e.into_inner());
            match &mut pacing {
                Paced::Device(pump) => pump.pump(&mut m, &mut sink) as u64,
                Paced::Logical {
                    now,
                    last,
                    max_catch_up,
                    clock,
                    rate,
                } => {
                    sink.maintain();
                    let r = sink.sample_rate();
                    if r != *rate {
                        *rate = r;
                        *clock = AudioClock::new(r, *max_catch_up);
                    }
                    if m.sample_rate() != r {
                        m.set_sample_rate(r);
                    }
                    let t = now();
                    let ticks = t.saturating_sub(*last);
                    *last = t.max(*last);
                    let frames = clock.advance(Duration::from_nanos(ticks.saturating_mul(100)));
                    let mut left = frames;
                    while left > 0 {
                        let n = left.min(CHUNK);
                        scratch.resize(2 * n as usize, 0.0);
                        m.render(&mut scratch);
                        for v in &mut scratch {
                            *v = v.clamp(-1.0, 1.0);
                        }
                        let taken = sink.write(&scratch) as u64;
                        shared.dropped.fetch_add(n - taken, Ordering::AcqRel);
                        left -= n;
                    }
                    frames
                }
            }
        };
        shared.rendered.fetch_add(rendered, Ordering::AcqRel);
        shared.rate.store(sink.sample_rate(), Ordering::Release);
        *shared.sink.lock().unwrap_or_else(|e| e.into_inner()) = sink.stats();
        if let Err(stop) = hle.do_work(&runtime) {
            hle.record_stop(StopRecord {
                addr: 0,
                name: "Hle::output",
                args: Vec::new(),
                stop,
            });
            return;
        }
        shared.wakes.fetch_add(1, Ordering::AcqRel);
    }
}

enum Paced {
    Device(RingPump),
    Logical {
        now: Box<dyn FnMut() -> u64 + Send>,
        last: u64,
        max_catch_up: Duration,
        clock: AudioClock,
        rate: u32,
    },
}
