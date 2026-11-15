//! The one mixer consumer that takes DirectSound to the host's audio output.
//!
//! [`Hle::spawn_output`] starts a thread that owns an [`AudioSink`] (a
//! `audio::output::DeviceSink` in the app, anything else in tests). It
//! replaces the discard renderers: one consumer renders, so audio time runs
//! once. Two pacings decide who renders and when packets complete:
//!
//! - [`Pacing::Device`]: the thread renders what the device wants and then
//!   completes what that finished (`Hle::do_work`). Real time: packets keep
//!   completing while the game sits in a loading loop, but when they
//!   complete relative to the game's own operations depends on host
//!   scheduling. Not for deterministic runs.
//! - [`Pacing::Logical`]: the runner drives audio time from its scheduler
//!   with [`Output::advance_to`]. That call renders the mixer on a fixed
//!   grid and completes packets synchronously, ordered with the game's
//!   DirectSound calls exactly as an override call is (the same guest memory
//!   and `DirectSound` locks). The thread only plays the rendered PCM: it
//!   never touches the mixer, guest memory or events. Guest-visible audio
//!   state then depends only on the order of guest operations and
//!   `advance_to` calls, never on host thread timing.
//!
//! Completion order is the same either way: each packet's completed size
//! and status reach guest memory, in submission order, before its event is
//! published; events are delivered with no guard held; stream callbacks,
//! being guest calls, wait for the next override call on a guest thread.
//!
//! The sink is opened on the output thread (`DeviceSink` is not `Send` on
//! macOS) and dropped there. [`Output::halt`] (or [`OutputConfig::halt_when`])
//! stops all further guest memory and event work, for a runner that has
//! faulted or stopped.
use crate::register::{Hle, StopRecord};
use crate::state::{Consumer, Lease, Stop};
use audio::resample::LinearResampler;
use audio::sink::{AudioSink, Pump as RingPump, SinkStats};
use runtime::Runtime;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Who drives audio time.
pub enum Pacing {
    /// The output thread keeps `target_latency_ms` of audio queued in the
    /// sink and completes packets after each render: audio time is the
    /// device's, which is real time. The mixer follows the sink's rate. A
    /// sink nobody drains stops audio time once it is full. Nondeterministic
    /// by nature (see the module docs).
    Device { target_latency_ms: u32 },
    /// The runner drives audio time with [`Output::advance_to`] at its
    /// scheduler points (each logical tick, vblank, or before a guest thread
    /// resumes), passing the guest's logical time in 100 ns ticks (the
    /// kernel's `Clock::counter`). Frame `n` is due `n / rate` after the
    /// logical time given at spawn (`origin`), at the mixer's rate at spawn;
    /// it is rendered in whole [`QUANTUM`]s, with completions after each.
    /// Rendered audio waits in a queue for the thread to play; at most
    /// `max_queued` of it is kept (the oldest beyond that is dropped and
    /// counted), so a runner ahead of real time does not grow it without
    /// bound. If logical time runs slower than real time the device
    /// underruns. Audio time stands still while the runner does not call
    /// `advance_to`.
    Logical { origin: u64, max_queued: Duration },
}

/// Settings for [`Hle::spawn_output`].
pub struct OutputConfig {
    /// How long the thread sleeps between wakes. With device pacing, also
    /// the worst-case delay before a finished packet is reported; keep it
    /// well under the latency target.
    pub period: Duration,
    pub pacing: Pacing,
    /// Polled before every guest-memory step (the thread's, and each
    /// `advance_to` quantum); once it returns true the output halts as for
    /// [`Output::halt`]. For a runner that keeps a flag for "faulted or
    /// stopped".
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
    /// Rendered frames never played: the sink had no room, or (logical
    /// pacing) the queue overflowed.
    pub dropped_frames: u64,
    /// Thread wakes completed.
    pub wakes: u64,
    /// The output halted ([`Output::halt`] or `halt_when`) and no longer
    /// touches guest memory or events.
    pub halted: bool,
    /// The sink's rate. Device pacing makes the mixer follow it; logical
    /// pacing keeps the mixer's rate and resamples to it.
    pub sample_rate: u32,
    /// The sink's own counters (underruns, frames played).
    pub sink: SinkStats,
}

/// Held across every guest-memory step; `halted` once halted.
struct Gate {
    halted: bool,
    when: Option<Box<dyn FnMut() -> bool + Send>>,
}

impl Gate {
    /// Still open (polling `when` first).
    fn open(&mut self) -> bool {
        if !self.halted && self.when.as_mut().is_some_and(|f| f()) {
            self.halted = true;
        }
        !self.halted
    }
}

struct Shared {
    stop: AtomicBool,
    gate: Mutex<Gate>,
    rendered: AtomicU64,
    dropped: AtomicU64,
    wakes: AtomicU64,
    rate: AtomicU32,
    sink: Mutex<SinkStats>,
    /// Logical pacing: rendered interleaved stereo waiting to be played.
    queue: Mutex<VecDeque<f32>>,
}

impl Shared {
    fn gate(&self) -> MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The logical grid [`Output::advance_to`] renders on.
struct Grid {
    origin: u64,
    /// Frames rendered since `origin`.
    done: u64,
    rate: u32,
    max_queued: usize,
}

/// The output thread [`Hle::spawn_output`] started. Dropping it stops and
/// joins the thread, closes the sink and gives the mixer back (another
/// consumer may then claim it).
pub struct Output {
    shared: Arc<Shared>,
    join: Option<std::thread::JoinHandle<()>>,
    hle: Arc<Hle>,
    runtime: Arc<Runtime>,
    lease: Lease,
    grid: Option<Mutex<Grid>>,
}

impl Output {
    pub fn stats(&self) -> OutputStats {
        let s = &self.shared;
        OutputStats {
            rendered_frames: s.rendered.load(Ordering::Acquire),
            dropped_frames: s.dropped.load(Ordering::Acquire),
            wakes: s.wakes.load(Ordering::Acquire),
            halted: s.gate().halted,
            sample_rate: s.rate.load(Ordering::Acquire),
            sink: *s.sink.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }

    /// The thread has exited (dropped, halted, or it hit a stop: see
    /// [`Hle::take_stops`], name `"Hle::output"`).
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(|j| j.is_finished())
    }

    /// Logical pacing: advances audio time to `logical` (100 ns ticks, the
    /// same clock as `Pacing::Logical::origin`). Renders every whole
    /// [`QUANTUM`] now due and, after each, completes what it finished:
    /// statuses written and events delivered before this returns, callbacks
    /// owed to the next override call. Returns the frames rendered.
    ///
    /// Call it from the runner at its scheduler points, with no guest
    /// memory guard held and no override running on this thread: it is
    /// ordered with the game's DirectSound calls by the same locks they
    /// take, so the guest sees the same result whatever the host threads
    /// do. Time going backwards renders nothing. A no-op (0) with device
    /// pacing or once halted.
    pub fn advance_to(&self, logical: u64) -> Result<u64, Stop> {
        let Some(grid) = &self.grid else {
            return Ok(0);
        };
        let mut g = grid.lock().unwrap_or_else(|e| e.into_inner());
        let due = due_frames(logical.saturating_sub(g.origin), g.rate);
        let mut rendered = 0;
        let mut block = vec![0.0f32; 2 * QUANTUM as usize];
        while g.done + QUANTUM <= due {
            let mut gate = self.shared.gate();
            if !gate.open() {
                break;
            }
            block.fill(0.0);
            self.hle
                .render_leased(&self.runtime, &mut block, &self.lease)?;
            drop(gate);
            g.done += QUANTUM;
            rendered += QUANTUM;
            self.shared.rendered.fetch_add(QUANTUM, Ordering::AcqRel);
            let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            q.extend(block.iter().map(|v| v.clamp(-1.0, 1.0)));
            let over = (q.len() / 2).saturating_sub(g.max_queued);
            if over > 0 {
                q.drain(..2 * over);
                self.shared.dropped.fetch_add(over as u64, Ordering::AcqRel);
            }
        }
        Ok(rendered)
    }

    /// Stops all guest work: waits for a completion step in progress, and
    /// after it returns neither the thread nor [`advance_to`](Self::advance_to)
    /// writes a packet status or signals an event again (the thread exits
    /// at its next wake). Call it when the runner faults or stops, before
    /// taking a snapshot or tearing down the event bridge. The mixer stays
    /// leased until the `Output` drops. Do not call it while holding the
    /// guest memory guard.
    pub fn halt(&self) {
        self.shared.gate().halted = true;
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

/// The logical grid, in frames: the mixer is rendered and completions
/// collected once per quantum (5.3 ms at 48 kHz).
pub const QUANTUM: u64 = 256;

impl Hle {
    /// Starts the output thread: `open` runs on it and returns the sink the
    /// mixer is played into (for the app,
    /// `audio::output::DeviceSink::open_default`). See [`Pacing`] for who
    /// renders.
    ///
    /// This leases the mixer as [`Consumer::Output`]: it fails if anything
    /// already consumes it (`set_frames_per_do_work`, a render pump,
    /// `advance_audio`, or another output thread), and they fail while this
    /// runs. Nothing can check for a consumer outside this crate, so do not
    /// also hand the mixer to the app's `AudioOut` or `DiscardRenderer`, or
    /// to `audio::Pump`.
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
        let (lease, mixer_rate) = {
            let mut ds = self.direct_sound();
            let lease = ds.lease_consumer(Consumer::Output)?;
            let rate = ds
                .mixer()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .sample_rate();
            (lease, rate)
        };
        let OutputConfig {
            period,
            pacing,
            halt_when,
        } = config;
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            gate: Mutex::new(Gate {
                halted: false,
                when: halt_when,
            }),
            rendered: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            wakes: AtomicU64::new(0),
            rate: AtomicU32::new(0),
            sink: Mutex::new(SinkStats::default()),
            queue: Mutex::new(VecDeque::new()),
        });
        let (mode, grid) = match pacing {
            Pacing::Device { target_latency_ms } => (Mode::Device(target_latency_ms), None),
            Pacing::Logical { origin, max_queued } => {
                let max_queued = (max_queued.as_secs_f64() * f64::from(mixer_rate)) as usize;
                (
                    Mode::Logical(mixer_rate),
                    Some(Mutex::new(Grid {
                        origin,
                        done: 0,
                        rate: mixer_rate,
                        max_queued: max_queued.max(QUANTUM as usize),
                    })),
                )
            }
        };
        let hle = Arc::clone(self);
        let rt = Arc::clone(&runtime);
        let sh = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("dsound-output".into())
            .spawn(move || {
                let sink = open();
                match mode {
                    Mode::Device(ms) => run_device(hle, rt, sink, period, ms, sh),
                    Mode::Logical(rate) => run_logical(sink, period, rate, sh),
                }
            })
            .expect("spawning the dsound output thread");
        Ok(Output {
            shared,
            join: Some(join),
            hle: Arc::clone(self),
            runtime,
            lease,
            grid,
        })
    }
}

enum Mode {
    Device(u32),
    /// The grid's (the mixer's) rate.
    Logical(u32),
}

fn publish_sink<S: AudioSink>(shared: &Shared, sink: &S) {
    shared.rate.store(sink.sample_rate(), Ordering::Release);
    *shared.sink.lock().unwrap_or_else(|e| e.into_inner()) = sink.stats();
}

/// Device pacing: render what the sink wants, then complete.
fn run_device<S: AudioSink>(
    hle: Arc<Hle>,
    runtime: Arc<Runtime>,
    mut sink: S,
    period: Duration,
    target_latency_ms: u32,
    shared: Arc<Shared>,
) {
    let mixer = Arc::clone(hle.direct_sound().mixer());
    let mut pump = RingPump::new(target_latency_ms);
    while !shared.stop.load(Ordering::Acquire) {
        std::thread::sleep(period);
        let rendered = {
            let mut m = mixer.lock().unwrap_or_else(|e| e.into_inner());
            pump.pump(&mut m, &mut sink) as u64
        };
        shared.rendered.fetch_add(rendered, Ordering::AcqRel);
        publish_sink(&shared, &sink);
        let result = {
            let mut gate = shared.gate();
            if !gate.open() {
                return;
            }
            hle.do_work(&runtime)
        };
        if let Err(stop) = result {
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

/// Logical pacing: play what `advance_to` rendered; nothing else.
fn run_logical<S: AudioSink>(mut sink: S, period: Duration, rate: u32, shared: Arc<Shared>) {
    let mut resampler: Option<(u32, LinearResampler)> = None;
    let mut out = Vec::new();
    while !shared.stop.load(Ordering::Acquire) {
        std::thread::sleep(period);
        if !shared.gate().open() {
            return;
        }
        sink.maintain();
        let to = sink.sample_rate();
        let free = sink.free_frames();
        // Source frames that fit the sink's free space at its rate.
        let take = if to == rate {
            free
        } else {
            (free as u64 * u64::from(rate) / u64::from(to.max(1))) as usize
        };
        let input: Vec<f32> = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            let n = take.min(q.len() / 2);
            q.drain(..2 * n).collect()
        };
        if !input.is_empty() {
            out.clear();
            let samples: &[f32] = if to == rate {
                resampler = None;
                &input
            } else {
                if resampler.as_ref().is_none_or(|(r, _)| *r != to) {
                    resampler = Some((to, LinearResampler::new(2, rate, to)));
                }
                resampler.as_mut().unwrap().1.process(&input, &mut out);
                &out
            };
            let taken = sink.write(samples);
            let short = (samples.len() / 2).saturating_sub(taken);
            shared.dropped.fetch_add(short as u64, Ordering::AcqRel);
        }
        publish_sink(&shared, &sink);
        shared.wakes.fetch_add(1, Ordering::AcqRel);
    }
}

/// Frames due `ticks` (100 ns) after the grid's origin, at `rate`.
fn due_frames(ticks: u64, rate: u32) -> u64 {
    (u128::from(ticks) * u128::from(rate) / 10_000_000) as u64
}
