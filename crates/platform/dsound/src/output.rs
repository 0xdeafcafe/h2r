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
//!
//! A second sink, the controller speaker ([`SideOutput`],
//! `docs/audio/playstation-audio.md`), plays the voices the pad router labelled.
//! The mixer renders both from the same frames (`Mixer::render_split`), on
//! the same clock; the side sink only has its own queue and resampler, so it
//! can underrun on its own. While the side sink is absent or has lost its
//! device the mixer renders one output and every voice stays on the main
//! mix: a routed sound is never lost.
use crate::register::{Hle, StopRecord};
use crate::state::{Consumer, Lease, Stop};
#[cfg(feature = "playback")]
use audio::{resample::LinearResampler, sink::Pump as RingPump};
use audio::sink::{AudioSink, SinkStats};
use runtime::Runtime;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Who drives audio time.
pub enum Pacing {
    /// The output thread keeps `target_latency_ms` of audio queued in the
    /// sink and completes packets after each render: audio time is the
    /// device's, which is real time. The mixer follows the sink's rate. A
    /// sink nobody drains stops audio time once it is full. Nondeterministic
    /// by nature (see the module docs).
    #[cfg(feature = "playback")]
    Device { target_latency_ms: u32 },
    /// The runner drives audio time with [`Output::advance_to`] at its
    /// scheduler points (each logical tick, vblank, or before a guest thread
    /// resumes), passing the guest's logical time in 100 ns ticks (the
    /// kernel's `Clock::counter`). Frame `n` is due `n / rate` after the
    /// logical time given at spawn (`origin`), at the mixer's rate at spawn;
    /// it is rendered in whole [`QUANTUM`]s, with completions after each.
    /// Rendered audio waits in a queue for the thread to play; at most
    /// `max_queued` of it is kept, raised when necessary to hold the
    /// playout's highest target plus a render quantum (the oldest beyond that is
    /// dropped and counted). If logical time runs slower than real time the
    /// device underruns. Audio time stands still while the runner does not
    /// call `advance_to`.
    ///
    /// `playout` is how the thread plays it: `None` writes everything to the
    /// sink as soon as it is rendered (exact samples, for tests); a
    /// [`Playout`] keeps a jitter buffer, which a real device needs.
    #[cfg(feature = "playback")]
    Logical {
        origin: u64,
        max_queued: Duration,
        playout: Option<Playout>,
    },
    /// Logical packet and cursor timing with no sample rendering or sink.
    Completions { origin: u64 },
}

/// The jitter buffer between logical audio time and a device.
///
/// A runner renders in bursts at its scheduler points, a game frame apart
/// (16.7 ms in menus, 33.3 ms in a level), and logical time trails wall time
/// by however long the frame's work took. The device pulls at its own steady
/// rate. Played the moment it is rendered, the queue sits at roughly zero
/// between bursts and every late frame is a dropout. So the thread:
///
/// - holds playback until `target_ms` of audio (sink plus queue) is waiting,
///   at the start and again whenever the device ran dry, so a dropout is one
///   clean gap rather than a run of clicks;
/// - after a dropout, raises the target by `step_ms` (up to `max_target_ms`),
///   and lets it fall back by 1 ms a second after `relax_after` of clean
///   play;
/// - trims the playback rate by at most [`MAX_RATE_TRIM`] (0.5%) to hold the
///   average fill at the target, which absorbs the device's clock drift and
///   bleeds off the extra latency a hitch leaves behind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playout {
    pub target_ms: u32,
    pub max_target_ms: u32,
    pub step_ms: u32,
    pub relax_after: Duration,
}

impl Default for Playout {
    /// 50 ms, growing to at most 150 ms.
    fn default() -> Self {
        Self {
            target_ms: 50,
            max_target_ms: 150,
            step_ms: 25,
            relax_after: Duration::from_secs(30),
        }
    }
}

/// The most [`Playout`] speeds up or slows down playback: 0.5%, about 9
/// cents, below what is heard on game audio.
pub const MAX_RATE_TRIM: f64 = 0.005;

/// What the jitter buffer is doing ([`OutputStats::playout`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayoutStats {
    /// Audio waiting (sink plus queue) at the last wake, ms.
    pub fill_ms: f64,
    /// Lowest and highest fill seen since the last [`Output::take_window`].
    pub min_fill_ms: f64,
    pub max_fill_ms: f64,
    /// The current target, ms.
    pub target_ms: f64,
    /// Playing (false while it waits for the target).
    pub primed: bool,
    /// Times the device ran dry while playing.
    pub starves: u64,
    /// The playback-rate trim in use, parts per million (+ is faster).
    pub trim_ppm: i32,
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
    /// The controller speaker, if one was asked for.
    pub side: Option<SideOutput>,
}

/// Opens the side sink on the output thread; `None` when its device is not
/// there (the main mix then keeps every voice).
pub type SideOpener = Box<dyn FnOnce() -> Option<Box<dyn AudioSink>> + Send>;

/// The controller-speaker output: a second sink for voices labelled
/// [`audio::VoiceOutput::Side`] or `Both`.
pub struct SideOutput {
    pub open: SideOpener,
    /// Called on the output thread with `true` when the side sink starts
    /// taking the routed voices and `false` when it stops (its device went
    /// away, or the output is dropped): the place to switch the controller's
    /// audio path to its speaker and back.
    pub on_live: Option<Box<dyn FnMut(bool) + Send>>,
}

impl Default for OutputConfig {
    /// 5 ms wakes, device pacing at the audio crate's 40 ms target.
    fn default() -> Self {
        Self {
            period: Duration::from_millis(5),
            #[cfg(feature = "playback")]
            pacing: Pacing::Device {
                target_latency_ms: audio::sink::DEFAULT_TARGET_LATENCY_MS,
            },
            #[cfg(not(feature = "playback"))]
            pacing: Pacing::Completions { origin: 0 },
            halt_when: None,
            side: None,
        }
    }
}

/// Counters of the side (controller speaker) output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SideStats {
    /// The side sink is open and connected: routed voices play there.
    pub live: bool,
    /// Frames of side mix rendered (only while live).
    pub rendered_frames: u64,
    /// Side frames never played: no room, or its queue overflowed.
    pub dropped_frames: u64,
    /// The side sink's rate (0 before it opens or if it is absent).
    pub sample_rate: u32,
    pub sink: SinkStats,
}

/// Counters of a running [`Output`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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
    /// The controller speaker, when [`OutputConfig::side`] asked for one.
    pub side: Option<SideStats>,
    /// The jitter buffer (logical pacing with a [`Playout`]).
    pub playout: Option<PlayoutStats>,
    /// Wakes that came more than [`LATE_WAKE`] after the one before: the
    /// thread was held up (a device probe, the host).
    pub late_wakes: u64,
    /// The longest time between two wakes since the last
    /// [`Output::take_window`], µs.
    pub max_wake_gap_us: u64,
}

/// The main sink's counters and their most recent changes, as sampled by
/// the output thread. Times are monotonic microseconds since output creation.
#[derive(Clone, Copy, Debug)]
pub struct OutputHealth {
    pub elapsed_us: u64,
    pub sink: Option<SinkStats>,
    pub last_underrun_us: Option<u64>,
    pub playout: Option<PlayoutStats>,
    pub last_playout_starve_us: Option<u64>,
    pub dropped_frames: u64,
    pub last_drop_us: Option<u64>,
}

/// Read-only output telemetry. Holding it does not keep the output thread
/// running or take a guest memory or mixer lock.
#[derive(Clone)]
pub struct OutputHealthProbe {
    shared: Arc<Shared>,
    clock_only: bool,
}

impl OutputHealthProbe {
    pub fn elapsed_us(&self) -> u64 {
        self.shared.elapsed_us()
    }

    pub fn read(&self) -> OutputHealth {
        let s = &self.shared;
        let sink = *s.sink.lock().unwrap_or_else(|e| e.into_inner());
        let playout = *s.playout.lock().unwrap_or_else(|e| e.into_inner());
        let dropped = *s.dropped.lock().unwrap_or_else(|e| e.into_inner());
        OutputHealth {
            elapsed_us: self.elapsed_us(),
            sink: (!self.clock_only).then_some(sink.stats),
            last_underrun_us: sink.last_increase_us,
            playout: playout.stats,
            last_playout_starve_us: playout.last_increase_us,
            dropped_frames: dropped.stats,
            last_drop_us: dropped.last_increase_us,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct TimedStats<T> {
    stats: T,
    last_increase_us: Option<u64>,
}

impl<T> TimedStats<T> {
    #[cfg(feature = "playback")]
    fn publish(&mut self, stats: T, count: impl Fn(&T) -> u64, elapsed_us: u64) {
        if count(&stats) > count(&self.stats) {
            self.last_increase_us = Some(elapsed_us);
        }
        self.stats = stats;
    }
}

/// A wake gap over the period plus this is a late wake.
pub const LATE_WAKE: Duration = Duration::from_millis(10);

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
    dropped: Mutex<TimedStats<u64>>,
    wakes: AtomicU64,
    rate: AtomicU32,
    started: Instant,
    sink: Mutex<TimedStats<SinkStats>>,
    /// Logical pacing: rendered interleaved stereo waiting to be played.
    queue: Mutex<VecDeque<f32>>,
    /// A side output was configured.
    side_wanted: bool,
    /// The side sink takes the routed voices: render with a side output.
    side_live: AtomicBool,
    /// Rendered side mix waiting to be played (both pacings).
    side_queue: Mutex<VecDeque<f32>>,
    side_rendered: AtomicU64,
    side_dropped: AtomicU64,
    side_rate: AtomicU32,
    side_sink: Mutex<SinkStats>,
    playout: Mutex<TimedStats<Option<PlayoutStats>>>,
    late_wakes: AtomicU64,
    max_wake_gap_us: AtomicU64,
}

impl Shared {
    fn elapsed_us(&self) -> u64 {
        self.started.elapsed().as_micros() as u64
    }

    fn gate(&self) -> MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[cfg(feature = "playback")]
    fn note_dropped(&self, frames: u64) {
        if frames > 0 {
            let mut dropped = self.dropped.lock().unwrap_or_else(|e| e.into_inner());
            dropped.stats += frames;
            dropped.last_increase_us = Some(self.elapsed_us());
        }
    }

    /// Queues rendered side mix (clamped), keeping at most `max` frames.
    #[cfg(feature = "playback")]
    fn push_side(&self, block: &[f32], max: usize) {
        let mut q = self.side_queue.lock().unwrap_or_else(|e| e.into_inner());
        q.extend(block.iter().map(|v| v.clamp(-1.0, 1.0)));
        let over = (q.len() / 2).saturating_sub(max);
        if over > 0 {
            q.drain(..2 * over);
            self.side_dropped.fetch_add(over as u64, Ordering::AcqRel);
        }
        self.side_rendered
            .fetch_add((block.len() / 2) as u64, Ordering::AcqRel);
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

#[cfg(feature = "playback")]
type SampleTap = Box<dyn FnMut(&[f32]) + Send>;

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
    #[cfg(feature = "playback")]
    tap: Option<Mutex<SampleTap>>,
    clock_only: bool,
}

impl Output {
    /// Observe logical-time PCM before the device queue can drop old samples.
    /// Clock-only output has no PCM. The tap must not reenter DirectSound.
    #[cfg(feature = "playback")]
    pub fn tap_samples(&mut self, tap: impl FnMut(&[f32]) + Send + 'static) {
        self.tap = Some(Mutex::new(Box::new(tap)));
    }

    pub fn health_probe(&self) -> OutputHealthProbe {
        OutputHealthProbe {
            shared: Arc::clone(&self.shared),
            clock_only: self.clock_only,
        }
    }

    pub fn stats(&self) -> OutputStats {
        let s = &self.shared;
        OutputStats {
            rendered_frames: s.rendered.load(Ordering::Acquire),
            dropped_frames: s.dropped.lock().unwrap_or_else(|e| e.into_inner()).stats,
            wakes: s.wakes.load(Ordering::Acquire),
            halted: s.gate().halted,
            sample_rate: s.rate.load(Ordering::Acquire),
            sink: s.sink.lock().unwrap_or_else(|e| e.into_inner()).stats,
            side: s.side_wanted.then(|| SideStats {
                live: s.side_live.load(Ordering::Acquire),
                rendered_frames: s.side_rendered.load(Ordering::Acquire),
                dropped_frames: s.side_dropped.load(Ordering::Acquire),
                sample_rate: s.side_rate.load(Ordering::Acquire),
                sink: *s.side_sink.lock().unwrap_or_else(|e| e.into_inner()),
            }),
            playout: s.playout.lock().unwrap_or_else(|e| e.into_inner()).stats,
            late_wakes: s.late_wakes.load(Ordering::Acquire),
            max_wake_gap_us: s.max_wake_gap_us.load(Ordering::Acquire),
        }
    }

    /// [`stats`](Self::stats), then starts a new window for the windowed
    /// fields (the fill range, the longest wake gap): for a periodic report.
    pub fn take_window(&self) -> OutputStats {
        let stats = self.stats();
        let s = &self.shared;
        s.max_wake_gap_us.store(0, Ordering::Release);
        if let Some(p) = s.playout.lock().unwrap_or_else(|e| e.into_inner()).stats.as_mut() {
            p.min_fill_ms = p.fill_ms;
            p.max_fill_ms = p.fill_ms;
        }
        stats
    }

    /// The thread has exited (dropped, halted, or it hit a stop: see
    /// [`Hle::take_stops`], name `"Hle::output"`).
    pub fn is_finished(&self) -> bool {
        if self.clock_only {
            self.shared.gate().halted
        } else {
            self.join.as_ref().is_none_or(|j| j.is_finished())
        }
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
        if g.done + QUANTUM > due {
            return Ok(0);
        }
        #[cfg(feature = "playback")]
        let mut samples = (!self.clock_only).then_some({
            ([0.0f32; 2 * QUANTUM as usize], [0.0f32; 2 * QUANTUM as usize])
        });
        while g.done + QUANTUM <= due {
            let mut gate = self.shared.gate();
            if !gate.open() {
                break;
            }
            #[cfg(feature = "playback")]
            if let Some((block, side)) = &mut samples {
                let side_live = self.shared.side_live.load(Ordering::Acquire);
                self.hle.render_leased(
                    &self.runtime,
                    block,
                    side_live.then_some(&mut side[..]),
                    &self.lease,
                )?;
                drop(gate);
                if let Some(tap) = &self.tap {
                    tap.lock().unwrap_or_else(|e| e.into_inner())(block);
                }
                if side_live {
                    self.shared.push_side(side, g.max_queued);
                }
                let over = push_queue(&self.shared.queue, block, g.max_queued);
                self.shared.note_dropped(over);
            } else {
                self.hle.advance_audio_leased(&self.runtime, QUANTUM, &self.lease)?;
            }
            #[cfg(not(feature = "playback"))]
            self.hle.advance_audio_leased(&self.runtime, QUANTUM, &self.lease)?;
            g.done += QUANTUM;
            rendered += QUANTUM;
            self.shared.rendered.fetch_add(QUANTUM, Ordering::AcqRel);
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

/// The logical output's snapshot section: its grid.
pub const SECTION: snapshot::Tag = snapshot::Tag::new("audioout");
const SECTION_VERSION: u32 = 1;

impl Output {
    /// What refuses a save of this output: device pacing renders on wall
    /// time, so no saved point in it can be found again.
    pub fn snapshot_blockers(&self) -> Vec<String> {
        match self.grid {
            Some(_) => Vec::new(),
            None => vec!["the audio output is paced by the device (wall time)".into()],
        }
    }

    /// Adds the `audioout` section: where the logical grid stands (its
    /// origin and the frames rendered since), so a load renders the next
    /// quantum at the same logical time. The audio rendered and not yet
    /// played is output only, and is not saved.
    pub fn save_snapshot(
        &self,
        out: &mut snapshot::SnapshotWriter,
    ) -> Result<(), crate::register::SnapshotError> {
        let Some(grid) = &self.grid else {
            return Err(crate::register::SnapshotError::Busy(
                self.snapshot_blockers(),
            ));
        };
        let g = grid.lock().unwrap_or_else(|e| e.into_inner());
        let Grid {
            origin,
            done,
            rate,
            // Keep: the loading output's setting.
            max_queued: _,
        } = &*g;
        let mut w = snapshot::codec::Writer::new();
        w.u64(*origin).u64(*done).u32(*rate);
        out.section(
            SECTION,
            SECTION_VERSION,
            snapshot::Codec::Stored,
            w.into_bytes(),
        )?;
        Ok(())
    }

    /// Puts the grid where the `audioout` section says. Refused, changing
    /// nothing, for a device-paced output or one at another rate.
    pub fn load_snapshot(
        &self,
        snapshot: &snapshot::Snapshot<'_>,
    ) -> Result<(), crate::register::SnapshotError> {
        use crate::register::SnapshotError;
        let bytes = snapshot.require(SECTION, SECTION_VERSION..=SECTION_VERSION)?;
        let mut r = snapshot::codec::Reader::new(&bytes, "audioout");
        let (origin, done, rate) = (r.u64()?, r.u64()?, r.u32()?);
        r.finish()?;
        let Some(grid) = &self.grid else {
            return Err(SnapshotError::Busy(self.snapshot_blockers()));
        };
        let mut g = grid.lock().unwrap_or_else(|e| e.into_inner());
        if g.rate != rate {
            return Err(SnapshotError::Mismatch(format!(
                "audio grid saved at {rate} Hz, this output runs at {} Hz",
                g.rate
            )));
        }
        g.origin = origin;
        g.done = done;
        Ok(())
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
        let clock_only = matches!(config.pacing, Pacing::Completions { .. });
        let (lease, mixer_rate) = {
            let mut ds = self.direct_sound();
            let lease = ds.lease_consumer(Consumer::Output)?;
            if clock_only {
                ds.set_clock_only();
            }
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
            side,
        } = config;
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            gate: Mutex::new(Gate {
                halted: false,
                when: halt_when,
            }),
            rendered: AtomicU64::new(0),
            dropped: Mutex::new(TimedStats::default()),
            wakes: AtomicU64::new(0),
            rate: AtomicU32::new(0),
            started: Instant::now(),
            sink: Mutex::new(TimedStats::default()),
            queue: Mutex::new(VecDeque::new()),
            side_wanted: side.is_some(),
            side_live: AtomicBool::new(false),
            side_queue: Mutex::new(VecDeque::new()),
            side_rendered: AtomicU64::new(0),
            side_dropped: AtomicU64::new(0),
            side_rate: AtomicU32::new(0),
            side_sink: Mutex::new(SinkStats::default()),
            playout: Mutex::new(TimedStats::default()),
            late_wakes: AtomicU64::new(0),
            max_wake_gap_us: AtomicU64::new(0),
        });
        #[cfg(feature = "playback")]
        let (mode, grid) = match pacing {
            Pacing::Device { target_latency_ms } => (Some(Mode::Device(target_latency_ms)), None),
            Pacing::Completions { origin } => {
                shared.rate.store(mixer_rate, Ordering::Release);
                (None, Some(Mutex::new(Grid {
                    origin,
                    done: 0,
                    rate: mixer_rate,
                    max_queued: 0,
                })))
            }
            Pacing::Logical {
                origin,
                max_queued,
                playout,
            } => (
                Some(Mode::Logical(mixer_rate, playout)),
                Some(Mutex::new(Grid {
                    origin,
                    done: 0,
                    rate: mixer_rate,
                    max_queued: logical_queue_limit(max_queued, playout, mixer_rate),
                })),
            ),
        };
        #[cfg(not(feature = "playback"))]
        let (join, grid) = {
            let _ = (period, open);
            let Pacing::Completions { origin } = pacing;
            shared.rate.store(mixer_rate, Ordering::Release);
            let grid = Grid { origin, done: 0, rate: mixer_rate, max_queued: 0 };
            (None, Some(Mutex::new(grid)))
        };
        #[cfg(feature = "playback")]
        let join = mode.map(|mode| {
            let hle = Arc::clone(self);
            let rt = Arc::clone(&runtime);
            let sh = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("dsound-output".into())
                .spawn(move || {
                    let sink = open();
                    let playout = match mode {
                        Mode::Logical(_, p) => p,
                        Mode::Device(_) => None,
                    };
                    let side = side.map(|s| Side::open(s, Arc::clone(&sh), playout));
                    match mode {
                        Mode::Device(ms) => run_device(hle, rt, sink, side, period, ms, sh),
                        Mode::Logical(rate, playout) => {
                            run_logical(sink, side, period, rate, playout, sh)
                        }
                    }
                })
                .expect("spawning the dsound output thread")
        });
        Ok(Output {
            shared,
            join,
            hle: Arc::clone(self),
            runtime,
            lease,
            grid,
            #[cfg(feature = "playback")]
            tap: None,
            clock_only,
        })
    }
}

#[cfg(feature = "playback")]
enum Mode {
    Device(u32),
    /// The grid's (the mixer's) rate.
    Logical(u32, Option<Playout>),
}

#[cfg(feature = "playback")]
fn publish_sink<S: AudioSink>(shared: &Shared, sink: &S) {
    shared.rate.store(sink.sample_rate(), Ordering::Release);
    shared.sink.lock().unwrap_or_else(|e| e.into_inner()).publish(
        sink.stats(),
        |s| s.underruns,
        shared.elapsed_us(),
    );
}

/// Device pacing: render what the sink wants, then complete.
#[cfg(feature = "playback")]
fn run_device<S: AudioSink>(
    hle: Arc<Hle>,
    runtime: Arc<Runtime>,
    mut sink: S,
    mut side: Option<Side>,
    period: Duration,
    target_latency_ms: u32,
    shared: Arc<Shared>,
) {
    let mixer = Arc::clone(hle.direct_sound().mixer());
    let mut pump = RingPump::new(target_latency_ms);
    let mut side_block = Vec::new();
    // The side queue holds a few latency targets: it only bridges the two
    // devices' callback timing.
    let side_max = |rate: u32| (u64::from(rate) * u64::from(target_latency_ms) * 4 / 1000) as usize;
    while !shared.stop.load(Ordering::Acquire) {
        std::thread::sleep(period);
        if let Some(s) = side.as_mut() {
            s.maintain();
        }
        let side_live = shared.side_live.load(Ordering::Acquire);
        let (rendered, rate) = {
            let mut m = mixer.lock().unwrap_or_else(|e| e.into_inner());
            let n = pump.pump_split(&mut m, &mut sink, side_live.then_some(&mut side_block));
            (n as u64, m.sample_rate())
        };
        shared.rendered.fetch_add(rendered, Ordering::AcqRel);
        if side_live && !side_block.is_empty() {
            shared.push_side(&side_block, side_max(rate).max(QUANTUM as usize));
        }
        if let Some(s) = side.as_mut() {
            s.play(rate);
        }
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
#[cfg(feature = "playback")]
fn run_logical<S: AudioSink>(
    mut sink: S,
    mut side: Option<Side>,
    period: Duration,
    rate: u32,
    playout: Option<Playout>,
    shared: Arc<Shared>,
) {
    let mut feed = Feed::new(playout);
    let mut last = std::time::Instant::now();
    while !shared.stop.load(Ordering::Acquire) {
        std::thread::sleep(period);
        let now = std::time::Instant::now();
        let gap = now.duration_since(last);
        last = now;
        if gap > period + LATE_WAKE {
            shared.late_wakes.fetch_add(1, Ordering::AcqRel);
        }
        shared
            .max_wake_gap_us
            .fetch_max(gap.as_micros() as u64, Ordering::AcqRel);
        if !shared.gate().open() {
            return;
        }
        sink.maintain();
        let short = feed.play(&mut sink, &shared.queue, rate);
        shared.note_dropped(short);
        if let Some(p) = feed.stats() {
            let mut slot = shared.playout.lock().unwrap_or_else(|e| e.into_inner());
            let stats = match slot.stats {
                Some(old) => PlayoutStats {
                    min_fill_ms: old.min_fill_ms.min(p.fill_ms),
                    max_fill_ms: old.max_fill_ms.max(p.fill_ms),
                    ..p
                },
                None => p,
            };
            slot.publish(Some(stats), |p| p.map_or(0, |p| p.starves), shared.elapsed_us());
        }
        if let Some(s) = side.as_mut() {
            s.maintain();
            s.play(rate);
        }
        publish_sink(&shared, &sink);
        shared.wakes.fetch_add(1, Ordering::AcqRel);
    }
}

/// Moves queued interleaved stereo at one rate into a sink at its own,
/// through a jitter buffer when it has a [`Playout`].
#[cfg(feature = "playback")]
#[derive(Default)]
struct Feed {
    resampler: Option<(u32, LinearResampler)>,
    out: Vec<f32>,
    input: Vec<f32>,
    playout: Option<Playout>,
    jitter: Jitter,
}

/// [`Playout`]'s state.
#[cfg(feature = "playback")]
#[derive(Default)]
struct Jitter {
    primed: bool,
    /// The sink's underrun count when last looked at.
    underruns: u64,
    target_ms: f64,
    /// Smoothed fill, ms.
    mean_ms: f64,
    fill_ms: f64,
    trim: f64,
    starves: u64,
    /// Device frames played when playback started or the target last relaxed.
    calm_since: u64,
}

#[cfg(feature = "playback")]
impl Feed {
    fn new(playout: Option<Playout>) -> Self {
        Self {
            playout,
            jitter: Jitter {
                target_ms: playout.map_or(0.0, |p| f64::from(p.target_ms)),
                ..Jitter::default()
            },
            ..Self::default()
        }
    }

    fn stats(&self) -> Option<PlayoutStats> {
        let j = &self.jitter;
        self.playout.map(|_| PlayoutStats {
            fill_ms: j.fill_ms,
            min_fill_ms: j.fill_ms,
            max_fill_ms: j.fill_ms,
            target_ms: j.target_ms,
            primed: j.primed,
            starves: j.starves,
            trim_ppm: (j.trim * 1e6).round() as i32,
        })
    }

    /// Plays as much of `queue` (frames at `rate`) as fits the sink's free
    /// space, or with a [`Playout`] as much as it allows; returns the frames
    /// the sink refused.
    fn play(&mut self, sink: &mut dyn AudioSink, queue: &Mutex<VecDeque<f32>>, rate: u32) -> u64 {
        let to = sink.sample_rate().max(1);
        let rate = rate.max(1);
        let nominal = f64::from(rate) / f64::from(to);
        let mut step = nominal;
        if let Some(p) = self.playout {
            let queued = queue.lock().unwrap_or_else(|e| e.into_inner()).len() / 2;
            let fill_ms = sink.queued_frames() as f64 * 1000.0 / f64::from(to)
                + queued as f64 * 1000.0 / f64::from(rate);
            let stats = sink.stats();
            let j = &mut self.jitter;
            j.fill_ms = fill_ms;
            if stats.underruns > j.underruns {
                if j.primed {
                    // Ran dry while playing: start over with more margin.
                    j.primed = false;
                    j.starves += 1;
                    j.target_ms =
                        (j.target_ms + f64::from(p.step_ms)).min(f64::from(p.max_target_ms));
                }
                j.underruns = stats.underruns;
            }
            // Clean play for `relax_after`: the target comes back down 1 ms
            // per second of it.
            let relax = (p.relax_after.as_secs_f64() * f64::from(to)) as u64;
            let second = u64::from(to);
            if j.primed && stats.played_frames >= j.calm_since + relax + second {
                j.target_ms = (j.target_ms - 1.0).max(f64::from(p.target_ms));
                j.calm_since += second;
            }
            if !j.primed {
                if fill_ms < j.target_ms {
                    return 0;
                }
                j.primed = true;
                j.mean_ms = fill_ms;
                j.calm_since = stats.played_frames;
            }
            // Hold the average at the target: a little faster when above it.
            j.mean_ms += (fill_ms - j.mean_ms) * 0.02;
            let err = (j.mean_ms - j.target_ms) / j.target_ms.max(1.0);
            j.trim = (err * 0.02).clamp(-MAX_RATE_TRIM, MAX_RATE_TRIM);
            step = nominal * (1.0 + j.trim);
        }
        let free = sink.free_frames();
        // Source frames that fit the sink's free space at its rate (u128:
        // an unbounded sink reports usize::MAX free frames), less one for
        // the frame the resampler holds back.
        let take = if step == 1.0 {
            free
        } else {
            let frames = free as f64 * step;
            if frames >= usize::MAX as f64 {
                usize::MAX
            } else {
                (frames as usize).saturating_sub(1)
            }
        };
        self.input.clear();
        {
            let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
            let n = take.min(q.len() / 2);
            self.input.extend(q.drain(..2 * n));
        }
        if self.input.is_empty() {
            return 0;
        }
        self.out.clear();
        let samples: &[f32] = if step == 1.0 && self.playout.is_none() {
            self.resampler = None;
            &self.input
        } else {
            if self.resampler.as_ref().is_none_or(|(r, _)| *r != to) {
                self.resampler = Some((to, LinearResampler::new(2, rate, to)));
            }
            let r = &mut self.resampler.as_mut().unwrap().1;
            r.set_step(step);
            r.process(&self.input, &mut self.out);
            &self.out
        };
        let taken = sink.write(samples);
        (samples.len() / 2).saturating_sub(taken) as u64
    }
}

/// The side sink on the output thread, and whether it is live.
#[cfg(feature = "playback")]
struct Side {
    sink: Option<Box<dyn AudioSink>>,
    on_live: Option<Box<dyn FnMut(bool) + Send>>,
    live: bool,
    feed: Feed,
    shared: Arc<Shared>,
}

#[cfg(feature = "playback")]
impl Side {
    fn open(config: SideOutput, shared: Arc<Shared>, playout: Option<Playout>) -> Self {
        let mut side = Side {
            sink: (config.open)(),
            on_live: config.on_live,
            live: false,
            feed: Feed::new(playout),
            shared,
        };
        side.maintain();
        side
    }

    /// Services the sink and follows its device: live while it is
    /// connected. Going down drops the side mix still queued, since the
    /// voices it held are back on the main mix from the next render.
    fn maintain(&mut self) {
        let Some(sink) = self.sink.as_mut() else {
            return;
        };
        sink.maintain();
        let (live, rate) = (sink.connected(), sink.sample_rate());
        self.shared.side_rate.store(rate, Ordering::Release);
        self.set_live(live);
    }

    fn set_live(&mut self, live: bool) {
        if live == self.live {
            return;
        }
        self.live = live;
        self.shared.side_live.store(live, Ordering::Release);
        if !live {
            self.shared
                .side_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
        }
        if let Some(f) = self.on_live.as_mut() {
            f(live);
        }
    }

    /// Plays the queued side mix (rendered at `rate`).
    fn play(&mut self, rate: u32) {
        let Some(sink) = self.sink.as_mut() else {
            return;
        };
        if self.live {
            let short = self.feed.play(sink.as_mut(), &self.shared.side_queue, rate);
            self.shared.side_dropped.fetch_add(short, Ordering::AcqRel);
        }
        *self
            .shared
            .side_sink
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = sink.stats();
    }
}

#[cfg(feature = "playback")]
impl Drop for Side {
    fn drop(&mut self) {
        self.set_live(false);
    }
}

/// While priming, the sink ring can be empty. The queue alone must be
/// able to reach the highest target without discarding the render quantum
/// that crosses it.
#[cfg(feature = "playback")]
fn logical_queue_limit(max_queued: Duration, playout: Option<Playout>, rate: u32) -> usize {
    let queue_frames = (max_queued.as_secs_f64() * f64::from(rate)) as usize;
    let target_frames = playout.map_or(0, |p| {
        let ms = f64::from(p.target_ms.max(p.max_target_ms));
        let frames = (ms * f64::from(rate) / 1000.0).ceil() as usize;
        frames.saturating_add(QUANTUM as usize)
    });
    queue_frames.max(QUANTUM as usize).max(target_frames)
}

/// Queues rendered interleaved stereo (clamped), keeping at most `max`
/// frames; returns the oldest frames dropped to make room.
#[cfg(feature = "playback")]
fn push_queue(queue: &Mutex<VecDeque<f32>>, block: &[f32], max: usize) -> u64 {
    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
    q.extend(block.iter().map(|v| v.clamp(-1.0, 1.0)));
    let over = (q.len() / 2).saturating_sub(max);
    if over > 0 {
        q.drain(..2 * over);
    }
    over as u64
}

/// Frames due `ticks` (100 ns) after the grid's origin, at `rate`.
fn due_frames(ticks: u64, rate: u32) -> u64 {
    (u128::from(ticks) * u128::from(rate) / 10_000_000) as u64
}

/// A real-time simulation of logical pacing on a virtual clock: a guest that
/// renders in bursts at its frame boundaries (held to wall time the way
/// `h2 play`'s pacer holds it), the output thread's wakes, and a device that
/// pulls fixed callbacks at its own rate. Deterministic: no threads, no
/// sleeps.
#[cfg(all(test, feature = "playback"))]
mod sim {
    use super::*;
    use audio::sink::{Consumer, Producer, ring};

    /// The mixer's (logical) rate, as `boot::dsound_glue` uses.
    const MIX: u32 = 48_000;

    struct SimSink {
        rate: u32,
        prod: Producer,
        cons: Consumer,
        /// Left channel of everything played, silence included.
        played: Vec<f32>,
    }

    impl SimSink {
        fn new(rate: u32) -> Self {
            let (prod, cons) = ring(16_384);
            Self {
                rate,
                prod,
                cons,
                played: Vec::new(),
            }
        }
        fn callback(&mut self, frames: usize) {
            let mut buf = vec![0.0; frames * 2];
            self.cons.read(&mut buf);
            self.played.extend(buf.iter().step_by(2));
        }
    }

    impl AudioSink for SimSink {
        fn sample_rate(&self) -> u32 {
            self.rate
        }
        fn queued_frames(&self) -> usize {
            self.prod.queued_frames()
        }
        fn free_frames(&self) -> usize {
            self.prod.free_frames()
        }
        fn write(&mut self, samples: &[f32]) -> usize {
            self.prod.write(samples)
        }
        fn stats(&self) -> SinkStats {
            self.prod.stats()
        }
    }

    /// A tiny deterministic generator for jitter.
    struct Lcg(u64);
    impl Lcg {
        fn below(&mut self, n: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) % n.max(1)
        }
    }

    #[derive(Clone, Copy)]
    struct Scenario {
        /// Guest frame period, µs (16_667 at 60 Hz, 33_333 at 30 Hz).
        frame_us: u64,
        /// Guest work per frame, µs: wall time during which logical time
        /// stands still (CPU work, a blocking present).
        work_us: (u64, u64),
        /// Every this often, one frame takes `hitch_us` more.
        hitch: Option<(u64, u64)>,
        /// Output thread wake period and jitter, µs.
        wake_us: (u64, u64),
        /// Every this often a wake stalls this long (a device probe).
        stall: Option<(u64, u64)>,
        device_rate: u32,
        callback_frames: usize,
        seconds: u64,
    }

    impl Scenario {
        fn menu() -> Self {
            Self {
                frame_us: 16_667,
                work_us: (3_000, 15_000),
                hitch: None,
                wake_us: (5_000, 1_500),
                stall: Some((500_000, 15_000)),
                device_rate: 48_000,
                callback_frames: 512,
                seconds: 20,
            }
        }
        fn level() -> Self {
            Self {
                frame_us: 33_333,
                work_us: (8_000, 31_000),
                ..Self::menu()
            }
        }
    }

    #[derive(Debug, Default)]
    struct Outcome {
        underruns: u64,
        underrun_frames: u64,
        /// Times the jitter buffer ran dry while playing (one gap each).
        starves: u64,
        /// Sample-to-sample jumps in the played signal after it started.
        discontinuities: u64,
        dropped: u64,
        /// Frames the guest rendered and frames of it the device played.
        produced: u64,
        played_signal: u64,
        /// Mean ring + queue fill at the device rate, ms.
        mean_fill_ms: f64,
    }

    /// The test signal: smooth, never exactly zero, so silence and jumps show.
    fn signal(n: u64) -> f32 {
        0.3 + 0.2 * (n as f64 * 2.0 * std::f64::consts::PI * 220.0 / f64::from(MIX)).sin() as f32
    }

    fn run(s: Scenario, mut feed: Feed) -> Outcome {
        let mut rng = Lcg(0x5eed);
        let queue = Mutex::new(VecDeque::new());
        let max_queued = (MIX / 4) as usize;
        let mut sink = SimSink::new(s.device_rate);
        let end = s.seconds * 1_000_000;
        // Guest: logical time `l` (µs since origin), the pacer's origin
        // (logical, wall), when the frame in progress finishes.
        let mut l = 0u64;
        let mut origin = (0u64, 0u64);
        let mut produced = 0u64;
        let mut busy_until = 0u64;
        let mut next_hitch = s.hitch.map(|(every, _)| every);
        let mut next_produce = 0u64;
        let mut next_wake = s.wake_us.0;
        let mut next_stall = s.stall.map(|(every, _)| every);
        let mut callbacks = 0u64;
        let cb_at = |n: u64| n * s.callback_frames as u64 * 1_000_000 / u64::from(s.device_rate);
        let mut out = Outcome::default();
        let (mut fill_sum, mut fill_n) = (0f64, 0u64);
        loop {
            let next_cb = cb_at(callbacks + 1);
            let t = next_produce.min(next_wake).min(next_cb);
            if t >= end {
                break;
            }
            if t == next_cb {
                sink.callback(s.callback_frames);
                callbacks += 1;
                let q = queue.lock().unwrap().len() / 2;
                let fill = sink.queued_frames() as f64
                    + q as f64 * f64::from(s.device_rate) / f64::from(MIX);
                fill_sum += fill * 1000.0 / f64::from(s.device_rate);
                fill_n += 1;
            } else if t == next_produce {
                // `advance_to(l)`: every whole quantum due, into the queue.
                let due = l * u64::from(MIX) / 1_000_000 / QUANTUM * QUANTUM;
                let mut block = Vec::new();
                while produced < due {
                    block.push(signal(produced));
                    block.push(signal(produced));
                    produced += 1;
                }
                out.dropped += push_queue(&queue, &block, max_queued);
                // The frame's work, then the idle advance to the next
                // vblank; the pacer sleeps until wall time reaches it (less
                // the 2 ms lead), and forgives a lag over 100 ms.
                let mut work = s.work_us.0 + rng.below(s.work_us.1 - s.work_us.0);
                if let (Some(at), Some((every, len))) = (next_hitch, s.hitch)
                    && t >= at
                {
                    work += len;
                    next_hitch = Some(at + every);
                }
                busy_until = t + work;
                l += s.frame_us;
                let wall_of = |l: u64, o: (u64, u64)| o.1 + (l - o.0);
                if busy_until > wall_of(l, origin) + 100_000 {
                    origin = (l, busy_until);
                }
                next_produce = busy_until.max(wall_of(l, origin).saturating_sub(2_000));
            } else {
                let _ = busy_until;
                let short = feed.play(&mut sink, &queue, MIX);
                out.dropped += short;
                let mut gap = s.wake_us.0 + rng.below(s.wake_us.1);
                if let (Some(at), Some((every, len))) = (next_stall, s.stall)
                    && t >= at
                {
                    gap += len;
                    next_stall = Some(at + every);
                }
                next_wake = t + gap;
            }
        }
        let st = sink.stats();
        out.underruns = st.underruns;
        out.underrun_frames = st.underrun_frames;
        out.produced = produced;
        out.starves = feed.jitter.starves;
        out.mean_fill_ms = fill_sum / fill_n.max(1) as f64;
        let start = sink.played.iter().position(|&v| v != 0.0).unwrap_or(0);
        let played = &sink.played[start..];
        out.played_signal = played.iter().filter(|&&v| v != 0.0).count() as u64;
        out.discontinuities = played
            .windows(2)
            .filter(|w| (w[1] - w[0]).abs() > 0.02)
            .count() as u64;
        out
    }

    #[test]
    fn immediate_playout_underruns_under_ordinary_frame_jitter() {
        for s in [Scenario::menu(), Scenario::level()] {
            let o = run(s, Feed::new(None));
            eprintln!("immediate: {o:?}");
            assert!(o.underruns > 0, "{o:?}");
        }
    }

    /// Playback started; nothing lost, nothing out of place.
    fn assert_clean(o: &Outcome, max_fill_ms: f64) {
        assert_eq!(o.underruns, 0, "{o:?}");
        assert_eq!(o.discontinuities, 0, "{o:?}");
        assert_eq!(o.dropped, 0, "{o:?}");
        // Everything rendered was played, but for what is still buffered.
        let behind = o.produced - o.played_signal;
        assert!(
            (behind as f64) < max_fill_ms * f64::from(MIX) / 1000.0,
            "{o:?}"
        );
        assert!(o.mean_fill_ms < max_fill_ms, "{o:?}");
    }

    #[test]
    fn the_jitter_buffer_plays_menus_and_levels_without_a_dropout() {
        for s in [Scenario::menu(), Scenario::level()] {
            let o = run(s, Feed::new(Some(Playout::default())));
            eprintln!("buffered: {o:?}");
            assert_clean(&o, 80.0);
        }
    }

    #[test]
    fn the_jitter_buffer_resamples_to_a_44k_device_cleanly() {
        let s = Scenario {
            device_rate: 44_100,
            ..Scenario::level()
        };
        let o = run(s, Feed::new(Some(Playout::default())));
        eprintln!("44.1k: {o:?}");
        assert_eq!(
            (o.underruns, o.discontinuities, o.dropped),
            (0, 0, 0),
            "{o:?}"
        );
        assert!(o.mean_fill_ms < 80.0, "{o:?}");
    }

    #[test]
    fn the_logical_audio_grid_keeps_advancing_through_present_2300() {
        for fps in [30, 60] {
            for present in 2299_u64..=2301 {
                let ticks = (present * 10_000_000).div_ceil(fps);
                assert_eq!(due_frames(ticks, MIX), present * u64::from(MIX) / fps);
            }
        }
    }

    #[test]
    fn last_underrun_time_tracks_counter_changes_only() {
        let mut stats = TimedStats::<SinkStats>::default();
        stats.publish(SinkStats::default(), |s| s.underruns, 100);
        assert_eq!(stats.last_increase_us, None);
        let underrun = SinkStats { underruns: 1, underrun_frames: 20, played_frames: 512 };
        stats.publish(underrun, |s| s.underruns, 200);
        stats.publish(SinkStats { played_frames: 1024, ..underrun }, |s| s.underruns, 300);
        assert_eq!(stats.last_increase_us, Some(200));
    }

    #[test]
    fn playout_can_prime_at_a_target_larger_than_the_queue_setting() {
        let rate = 1000;
        let playout = Some(Playout {
            target_ms: 500,
            max_target_ms: 550,
            ..Playout::default()
        });
        let limit = logical_queue_limit(Duration::from_millis(250), playout, rate);
        assert_eq!(limit, 550 + QUANTUM as usize);
        let queue = Mutex::new(VecDeque::new());
        let mut sink = SimSink::new(rate);
        let mut feed = Feed::new(playout);

        assert_eq!(push_queue(&queue, &[0.5; 998], limit), 0);
        assert_eq!(feed.play(&mut sink, &queue, rate), 0);
        assert!(!feed.jitter.primed);
        assert_eq!(push_queue(&queue, &[0.5; 2], limit), 0);
        assert_eq!(feed.play(&mut sink, &queue, rate), 0);
        assert!(feed.jitter.primed);
        assert_eq!(sink.queued_frames(), 499); // resampler holds the last frame
    }

    #[test]
    fn long_refill_preserves_sample_order_and_defers_target_relaxation() {
        let rate = 1000;
        let mut feed = Feed::new(Some(Playout {
            target_ms: 4,
            max_target_ms: 10,
            step_ms: 2,
            relax_after: Duration::from_millis(4),
        }));
        let queue = Mutex::new(VecDeque::new());
        let mut sink = SimSink::new(rate);
        let stereo = |frames: &[f32]| frames.iter().flat_map(|&s| [s, s]).collect::<Vec<_>>();

        assert_eq!(push_queue(&queue, &stereo(&[0.125, 0.25, 0.375, 0.5]), 16), 0);
        assert_eq!(feed.play(&mut sink, &queue, rate), 0);
        sink.callback(5);
        assert_eq!(sink.played, [0.125, 0.25, 0.375, 0.0, 0.0]);
        assert_eq!(feed.play(&mut sink, &queue, rate), 0);
        assert_eq!(feed.jitter.starves, 1);
        assert!(!feed.jitter.primed);

        sink.callback(2000);
        let start = sink.played.len();
        let next = [0.625, 0.75, 0.875, 1.0, 0.875, 0.75];
        assert_eq!(push_queue(&queue, &stereo(&next), 16), 0);
        assert_eq!(feed.play(&mut sink, &queue, rate), 0);
        sink.callback(6);
        assert_eq!(&sink.played[start..], &[0.5, 0.625, 0.75, 0.875, 1.0, 0.875]);

        assert_eq!(feed.play(&mut sink, &queue, rate), 0);
        assert_eq!(feed.jitter.starves, 1);
        assert_eq!(feed.jitter.target_ms, 6.0);
    }

    #[test]
    fn a_hitch_costs_a_clean_gap_until_the_buffer_has_grown_for_it() {
        // A 100 ms hitch every 4 s, at 30 Hz.
        let s = Scenario {
            hitch: Some((4_000_000, 100_000)),
            seconds: 30,
            ..Scenario::level()
        };
        let o = run(s, Feed::new(Some(Playout::default())));
        eprintln!("hitch: {o:?}");
        // The first hitches find the buffer short; the target grows by a
        // step each time, so after two none of the 7 does.
        assert!((1..=2).contains(&o.starves), "{o:?}");
        // A dropout is one gap: in and out, not a run of clicks.
        assert!(o.discontinuities <= 2 * o.starves, "{o:?}");
        assert_eq!(o.dropped, 0, "{o:?}");
        assert!(o.mean_fill_ms < 160.0, "{o:?}");
    }
}
