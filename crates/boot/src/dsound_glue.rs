//! Installs crates/dsound through `dsound::register` (docs/dsound.md "Boot
//! integration") with a headless mixer. Completion events go through the
//! kernel, pinned with `Kernel::reference_dispatcher` so a closed handle
//! still gets its signal. The game's WMA codec runs natively
//! (`dsound::xwma`). Audio time is logical (docs/dsound.md "Boot
//! integration"): an output thread with `Pacing::Logical` into a
//! `NullSink`, which the runner drives with [`advance`] between slices, so
//! a run stays deterministic. When every guest thread waits and only
//! audio is still due to finish something, [`wake_idle`] moves logical
//! time on to the next audio quantum instead of reporting a deadlock.

use std::sync::{Arc, Mutex};

use dsound::{DirectSound, Hle, Layout};

struct KernelEvents(Arc<kernel::Kernel>);

/// A pinned kernel event; dropping it releases the pin.
struct Pin(kernel::DispatcherReference);

impl dsound::EventRef for Pin {
    fn signal(&mut self) -> Result<(), String> {
        self.0.signal_event().map(drop).map_err(|e| e.to_string())
    }
}

impl dsound::Events for KernelEvents {
    fn signal(&mut self, handle: u32) -> Result<(), String> {
        self.0.signal_event(handle).map(drop).map_err(|e| e.to_string())
    }
    fn reference(&mut self, handle: u32) -> Option<Result<Box<dyn dsound::EventRef>, String>> {
        Some(
            self.0
                .reference_dispatcher(handle)
                .map(|r| Box::new(Pin(r)) as Box<dyn dsound::EventRef>)
                .map_err(|e| e.to_string()),
        )
    }
}

/// The DirectSound HLE and its logical-time output.
pub struct Audio {
    pub hle: Arc<Hle>,
    pub output: dsound::Output,
    mixer: Arc<Mutex<audio::Mixer>>,
}

/// The mixer rate [`install`] uses.
const RATE: u64 = 48_000;
/// Logical ticks (100 ns) in which one more `dsound::output::QUANTUM` falls due.
pub const QUANTUM_TICKS: u64 = (dsound::output::QUANTUM * 10_000_000).div_ceil(RATE);
/// How much logical time [`wake_idle`] may add with no thread running in
/// between before the run is a deadlock after all: a looping buffer plays
/// for ever, and must not hide a real one.
pub const IDLE_LIMIT_TICKS: u64 = 10 * 10_000_000;

impl Audio {
    /// Something in the mixer can still finish and signal: a playing buffer
    /// or a stream with queued packets.
    pub fn pending(&self) -> bool {
        self.mixer.lock().unwrap_or_else(|e| e.into_inner()).active_voices() > 0
    }
}

/// Guest-thread slices the machine has scheduled so far. [`wake_idle`]'s
/// budget is reset only when this moves: a slice that only delivered a
/// periodic callback (vblank) ends in `SliceLimit` too, and must not refill
/// the budget while every thread stays blocked.
pub fn guest_slices(m: &kernel::Machine) -> u64 {
    m.schedule().len() as u64 + m.dropped_schedule()
}

/// The scheduler found no runnable thread and no timed wake. If audio is
/// pending (and `idle` logical ticks added this way since a thread last ran
/// are under [`IDLE_LIMIT_TICKS`]), advance logical time by one audio
/// quantum, render it (completing its packets and signalling their events)
/// and return true so the runner tries again. Between slices, with no
/// guest memory guard held.
pub fn wake_idle(a: &Audio, k: &kernel::Kernel, rt: &runtime::Runtime, idle: &mut u64) -> Result<bool, String> {
    if *idle >= IDLE_LIMIT_TICKS || !a.pending() {
        return Ok(false);
    }
    k.advance_time(rt, QUANTUM_TICKS).map_err(|e| format!("advance_time: {e:?}"))?;
    *idle += QUANTUM_TICKS;
    advance(a, k)?;
    Ok(true)
}

/// Opens the sink the logical-time output plays into, on the output thread
/// (`audio::output::DeviceSink` is not `Send` on macOS).
pub type SinkOpener = Box<dyn FnOnce() -> Box<dyn audio::sink::AudioSink> + Send>;

/// A boxed sink, so [`SinkOpener`] can pick one at run time.
struct BoxedSink(Box<dyn audio::sink::AudioSink>);

impl audio::sink::AudioSink for BoxedSink {
    fn sample_rate(&self) -> u32 {
        self.0.sample_rate()
    }
    fn queued_frames(&self) -> usize {
        self.0.queued_frames()
    }
    fn free_frames(&self) -> usize {
        self.0.free_frames()
    }
    fn write(&mut self, samples: &[f32]) -> usize {
        self.0.write(samples)
    }
    fn stats(&self) -> audio::sink::SinkStats {
        self.0.stats()
    }
    fn maintain(&mut self) {
        self.0.maintain()
    }
}

pub fn install(rt: &Arc<runtime::Runtime>, k: &Arc<kernel::Kernel>, xbe: &[u8]) -> anyhow::Result<Audio> {
    install_with_sink(rt, k, xbe, Box::new(|| Box::new(audio::NullSink::new(RATE as u32, RATE as usize / 4))))
}

/// [`install`] playing into the caller's sink (a host's audio device).
/// Pacing stays logical: audio time is the kernel's, so guest-visible audio
/// state is the same as a headless run; a host slower than real time is
/// heard as underruns, never as a different schedule.
pub fn install_with_sink(
    rt: &Arc<runtime::Runtime>,
    k: &Arc<kernel::Kernel>,
    xbe: &[u8],
    open: SinkOpener,
) -> anyhow::Result<Audio> {
    let mixer = Arc::new(Mutex::new(audio::Mixer::new(RATE as u32)));
    let hle = Hle::new(DirectSound::new(Arc::clone(&mixer), Layout::default()), Box::new(KernelEvents(Arc::clone(k))));
    let mut r = None;
    rt.edit_dispatch(|d| r = Some(dsound::register(d, &hle)));
    r.unwrap().map_err(|e| anyhow::anyhow!("dsound::register: {e:?}"))?;
    let wma = dsound::xwma::WmaHle::from_xbe(xbe).map_err(|e| anyhow::anyhow!("dsound::xwma: {e:?}"))?;
    let mut r = None;
    rt.edit_dispatch(|d| r = Some(dsound::xwma::register(d, &wma)));
    r.unwrap().map_err(|e| anyhow::anyhow!("dsound::xwma::register: {e:?}"))?;
    let output = hle
        .spawn_output(
            Arc::clone(rt),
            move || BoxedSink(open()),
            dsound::OutputConfig {
                period: std::time::Duration::from_millis(5),
                pacing: dsound::Pacing::Logical { origin: k.clock().counter(), max_queued: std::time::Duration::from_millis(250) },
                halt_when: None,
            },
        )
        .map_err(|e| anyhow::anyhow!("dsound output: {e:?}"))?;
    Ok(Audio { hle, output, mixer })
}

/// Bring audio time up to the kernel's logical time. Between slices, with
/// no guest memory guard held.
pub fn advance(a: &Audio, k: &kernel::Kernel) -> Result<(), String> {
    a.output.advance_to(k.clock().counter()).map(drop).map_err(|e| format!("{e:?}"))
}

pub fn describe(a: &Audio) -> String {
    let hle = &a.hle;
    format!(
        "dsound: pending callbacks {}; stops {:?}; signal failures {:?}; output {:?}",
        hle.pending_callbacks(),
        hle.take_stops(),
        hle.take_signal_failures(),
        a.output.stats()
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn one_quantum_tick_step_makes_a_quantum_due() {
        // dsound's due_frames: floor(ticks * rate / 1e7).
        let due = |t: u64| t * super::RATE / 10_000_000;
        for now in [0u64, 1, 53_333, 1_234_567, 99_999_999] {
            assert!(due(now + super::QUANTUM_TICKS) >= due(now) + dsound::output::QUANTUM, "{now}");
        }
    }
}
