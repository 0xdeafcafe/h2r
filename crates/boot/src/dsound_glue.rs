//! Installs crates/dsound through `dsound::register` (docs/dsound.md "Boot
//! integration") with a headless mixer. Completion events go through the
//! kernel, pinned with `Kernel::reference_dispatcher` so a closed handle
//! still gets its signal. The game's WMA codec runs natively
//! (`dsound::xwma`). Audio time is logical (docs/dsound.md "Boot
//! integration"): an output thread with `Pacing::Logical` into a
//! `NullSink`, which the runner drives with [`advance`] between slices, so
//! a run stays deterministic.

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
}

pub fn install(rt: &Arc<runtime::Runtime>, k: &Arc<kernel::Kernel>, xbe: &[u8]) -> anyhow::Result<Audio> {
    let mixer = Arc::new(Mutex::new(audio::Mixer::new(48_000)));
    let hle = Hle::new(DirectSound::new(mixer, Layout::default()), Box::new(KernelEvents(Arc::clone(k))));
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
            || audio::NullSink::new(48_000, 48_000 / 4),
            dsound::OutputConfig {
                period: std::time::Duration::from_millis(5),
                pacing: dsound::Pacing::Logical { origin: k.clock().counter(), max_queued: std::time::Duration::from_millis(250) },
                halt_when: None,
            },
        )
        .map_err(|e| anyhow::anyhow!("dsound output: {e:?}"))?;
    Ok(Audio { hle, output })
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
