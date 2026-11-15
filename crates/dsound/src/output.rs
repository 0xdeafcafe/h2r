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
//! Order, all on this thread:
//!
//! 1. render (under the mixer lock alone) and write to the sink: with
//!    [`Pacing::Device`] what the sink wants, with [`Pacing::Logical`] one
//!    [`QUANTUM`] of frames;
//! 2. collect completions exactly as `DirectSoundDoWork` does: each packet's
//!    completed size and status reach guest memory, in submission order,
//!    before its event is published;
//! 3. deliver the published events with no guard held. Stream callbacks are
//!    guest calls: they wait for the next override call on a guest thread,
//!    in order, as for any host-driven `Hle::do_work`.
//!
//! The sink is opened on the output thread (`DeviceSink` is not `Send` on
//! macOS) and dropped there. [`Output::halt`] (or [`OutputConfig::halt_when`])
//! makes the thread stop touching guest memory and events, for a runner that
//! has faulted or stopped.
use crate::register::{Hle, StopRecord};
use crate::state::{Consumer, Lease};
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
    /// Follow the guest's logical clock: `now` returns logical time in
    /// 100 ns ticks (the kernel's `Clock::counter`). Audio frame `n` is due
    /// at logical time `n / rate` from the thread's start, and the mixer is
    /// rendered in whole [`QUANTUM`]s on that fixed grid, with completions
    /// collected after each one. So the frames rendered, and the quantum in
    /// which each packet completes, depend only on logical time, never on
    /// when the host thread happens to wake: two wakes of 100 ms render
    /// exactly what one of 200 ms does. Nothing owed is ever discarded; at
    /// most `max_per_wake` is rendered per wake and the rest carries to the
    /// next (it bounds how long one wake holds the mixer, not audio time).
    ///
    /// What this cannot order is a guest submission made after logical time
    /// passed a quantum but before the thread rendered it: that needs the
    /// guest scheduler to drive the mixer, which this thread does not.
    /// Rendered audio the sink has no room for is dropped and counted in
    /// [`OutputStats::dropped_frames`]; if logical time runs slower than real
    /// time the device underruns instead. Audio time stops while logical
    /// time stands still: a game whose every thread waited on a stream event
    /// with no timeout would wait forever, so prefer `Device` pacing when a
    /// device is present.
    Logical {
        now: Box<dyn FnMut() -> u64 + Send>,
        max_per_wake: Duration,
    },
}

/// Settings for [`Hle::spawn_output`].
pub struct OutputConfig {
    /// How long the thread sleeps between wakes; also the worst-case delay
    /// before a finished packet is reported. Keep it well under the device
    /// pacing target.
    pub period: Duration,
    pub pacing: Pacing,
    /// Polled before every guest-memory step; once it returns true the
    /// thread halts as for [`Output::halt`]. For a runner that keeps a flag
    /// for "faulted or stopped".
    pub halt_when: Option<Box<dyn FnMut() -> bool + Send>>,
}

impl Default for OutputConfig {
    /// 5 ms wakes, device pacing at the audio crate's 40 ms target.
    fn default() -> Self {
        Self {
            period: Duration::from_millis(5),
            pacing: Pacing::Device {
                target_latency_ms: audio::sink::DEFAULT_TARGET_LATENCY_MS,
            },
            halt_when: None,
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
    /// The thread halted ([`Output::halt`] or `halt_when`) and no longer
    /// touches guest memory or events.
    pub halted: bool,
    /// The sink's rate, which the mixer follows.
    pub sample_rate: u32,
    /// The sink's own counters (underruns, frames played).
    pub sink: SinkStats,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    /// Held across every guest-memory step; `true` once halted.
    halted: Mutex<bool>,
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
    lease: Lease,
}

impl Output {
    pub fn stats(&self) -> OutputStats {
        let s = &self.shared;
        OutputStats {
            rendered_frames: s.rendered.load(Ordering::Acquire),
            dropped_frames: s.dropped.load(Ordering::Acquire),
            wakes: s.wakes.load(Ordering::Acquire),
            halted: *s.halted.lock().unwrap_or_else(|e| e.into_inner()),
            sample_rate: s.rate.load(Ordering::Acquire),
            sink: *s.sink.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
    /// The thread has exited (dropped, or it hit a stop: see
    /// [`Hle::take_stops`], name `"Hle::output"`).
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(|j| j.is_finished())
    }
    /// Stops the thread touching the guest: waits for a completion step in
    /// progress, and after it returns no packet status is written and no
    /// event is signalled by this thread again (it exits at its next wake).
    /// Call it when the runner faults or stops, before taking a snapshot or
    /// tearing down the event bridge. The mixer stays claimed until the
    /// `Output` drops. Do not call it while holding the guest memory guard.
    pub fn halt(&self) {
        *self.shared.halted.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        self.hle.direct_sound().release_lease(&self.lease);
    }
}

/// The logical-pacing grid, in frames: the mixer is rendered and
/// completions collected once per quantum (5.3 ms at 48 kHz).
pub const QUANTUM: u64 = 256;

impl Hle {
    /// Starts the output thread: `open` runs on it and returns the sink the
    /// mixer is rendered into (for the app,
    /// `audio::output::DeviceSink::open_default`). The mixer switches to the
    /// sink's sample rate, and follows it after a device change.
    ///
    /// This leases the mixer as [`Consumer::Output`]: it fails if anything
    /// already consumes it (`set_frames_per_do_work`, a render pump,
    /// `advance_audio`, or another output thread), and they fail while this
    /// runs. Nothing can check for a
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
        let lease = self.direct_sound().lease_consumer(Consumer::Output)?;
        let mut config = config;
        // The grid starts at the logical time of this call, not of the
        // thread's first run.
        let origin = match &mut config.pacing {
            Pacing::Logical { now, .. } => now(),
            Pacing::Device { .. } => 0,
        };
        let shared = Arc::new(Shared::default());
        let hle = Arc::clone(self);
        let sh = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("dsound-output".into())
            .spawn(move || run(hle, runtime, open(), config, origin, sh))
            .expect("spawning the dsound output thread");
        Ok(Output {
            shared,
            join: Some(join),
            hle: Arc::clone(self),
            lease,
        })
    }
}

fn run<S: AudioSink>(
    hle: Arc<Hle>,
    runtime: Arc<Runtime>,
    mut sink: S,
    mut config: OutputConfig,
    origin: u64,
    shared: Arc<Shared>,
) {
    let mixer = Arc::clone(hle.direct_sound().mixer());
    let mut scratch = Vec::new();
    let mut pacing = match config.pacing {
        Pacing::Device { target_latency_ms } => Paced::Device(RingPump::new(target_latency_ms)),
        Pacing::Logical { now, max_per_wake } => Paced::Logical {
            now,
            origin,
            done: 0,
            rate: sink.sample_rate(),
            max_per_wake,
        },
    };
    let mut halt_when = config.halt_when.take();
    // One guest-memory step, unless halted: completions, then events.
    let mut complete = || -> Result<bool, crate::state::Stop> {
        let mut halted = shared.halted.lock().unwrap_or_else(|e| e.into_inner());
        if !*halted && halt_when.as_mut().is_some_and(|f| f()) {
            *halted = true;
        }
        if *halted {
            return Ok(false);
        }
        hle.do_work(&runtime)?;
        Ok(true)
    };
    while !shared.stop.load(Ordering::Acquire) {
        std::thread::sleep(config.period);
        let mut rendered = 0u64;
        let result = match &mut pacing {
            Paced::Device(pump) => {
                rendered = {
                    let mut m = mixer.lock().unwrap_or_else(|e| e.into_inner());
                    pump.pump(&mut m, &mut sink) as u64
                };
                complete()
            }
            Paced::Logical {
                now,
                origin,
                done,
                rate,
                max_per_wake,
            } => {
                sink.maintain();
                let r = sink.sample_rate();
                let t = now();
                if r != *rate {
                    // A device change: restart the grid at the frame
                    // already rendered, at the new rate.
                    *origin = t;
                    *done = 0;
                    *rate = r;
                }
                let due = due_frames(t.saturating_sub(*origin), *rate);
                let budget = (max_per_wake.as_secs_f64() * f64::from(*rate)) as u64;
                let budget = budget.max(QUANTUM);
                let mut result = Ok(true);
                while *done + QUANTUM <= due && rendered + QUANTUM <= budget {
                    {
                        let mut m = mixer.lock().unwrap_or_else(|e| e.into_inner());
                        if m.sample_rate() != *rate {
                            m.set_sample_rate(*rate);
                        }
                        scratch.resize(2 * QUANTUM as usize, 0.0);
                        m.render(&mut scratch);
                    }
                    for v in &mut scratch {
                        *v = v.clamp(-1.0, 1.0);
                    }
                    let taken = sink.write(&scratch) as u64;
                    shared.dropped.fetch_add(QUANTUM - taken, Ordering::AcqRel);
                    *done += QUANTUM;
                    rendered += QUANTUM;
                    result = complete();
                    if !matches!(result, Ok(true)) {
                        break;
                    }
                }
                if rendered == 0 { complete() } else { result }
            }
        };
        shared.rendered.fetch_add(rendered, Ordering::AcqRel);
        shared.rate.store(sink.sample_rate(), Ordering::Release);
        *shared.sink.lock().unwrap_or_else(|e| e.into_inner()) = sink.stats();
        match result {
            Ok(true) => {
                shared.wakes.fetch_add(1, Ordering::AcqRel);
            }
            Ok(false) => return,
            Err(stop) => {
                hle.record_stop(StopRecord {
                    addr: 0,
                    name: "Hle::output",
                    args: Vec::new(),
                    stop,
                });
                return;
            }
        }
    }
}

/// Frames due `ticks` (100 ns) after the grid's origin, at `rate`.
fn due_frames(ticks: u64, rate: u32) -> u64 {
    (u128::from(ticks) * u128::from(rate) / 10_000_000) as u64
}

enum Paced {
    Device(RingPump),
    Logical {
        now: Box<dyn FnMut() -> u64 + Send>,
        /// Logical time of frame 0 of the grid.
        origin: u64,
        /// Frames rendered since `origin`.
        done: u64,
        rate: u32,
        max_per_wake: Duration,
    },
}
