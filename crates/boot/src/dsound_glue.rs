//! Installs crates/dsound through `dsound::register` (docs/dsound.md "Boot
//! integration") with a headless mixer. Completion events go through the
//! kernel, pinned with `Kernel::reference_dispatcher` so a closed handle
//! still gets its signal. No device renders the mixer, so playback advances
//! on the game's own DirectSoundDoWork calls (800 frames each, 48 kHz at 60
//! calls a second).

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

pub fn install(rt: &runtime::Runtime, k: &Arc<kernel::Kernel>) -> anyhow::Result<Arc<Hle>> {
    let mixer = Arc::new(Mutex::new(audio::Mixer::new(48_000)));
    let hle = Hle::new(DirectSound::new(mixer, Layout::default()), Box::new(KernelEvents(Arc::clone(k))));
    hle.direct_sound().set_frames_per_do_work(Some(800));
    let mut r = None;
    rt.edit_dispatch(|d| r = Some(dsound::register(d, &hle)));
    r.unwrap().map_err(|e| anyhow::anyhow!("dsound::register: {e:?}"))?;
    Ok(hle)
}

pub fn describe(hle: &Hle) -> String {
    format!(
        "dsound: pending callbacks {}; stops {:?}; signal failures {:?}",
        hle.pending_callbacks(),
        hle.take_stops(),
        hle.take_signal_failures()
    )
}
