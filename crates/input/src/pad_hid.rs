//! The DualSense over raw HID, next to the gamepad backends: the speaker
//! path and the mute button with its LED (`docs/playstation-audio.md`).
//!
//! GameController.framework and gilrs read the pad's buttons; neither passes
//! raw reports, so this opens the same device a second time, in shared mode
//! (macOS: an `IOHIDManager` opened with `kIOHIDOptionsTypeNone`, never
//! seizing it), reads only the mute bit out of its input reports and writes
//! only output reports whose other valid-flag bits are clear
//! ([`crate::dualsense::output_report`]): rumble, lightbar and player LEDs
//! stay with the other backends.
//!
//! [`PadLink`] is the per-device state machine: what to send when the device
//! opens, when the mute button is pressed, when the speaker is switched and
//! when the mute button itself is turned on or off ([`MuteSwitch`], the
//! Video screen's Mute Button row, applied at once). [`spawn`] runs it on a
//! thread of its own over any [`HidPad`]: the macOS IOKit one
//! ([`iokit_opener`]), or a fake in tests.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crate::dualsense::{self, MicMute, OutputRequest, Transport};

/// One opened controller.
pub trait HidPad {
    fn transport(&self) -> Transport;
    /// The next input report (report ID first), waiting at most `timeout`.
    /// `Ok(None)` if none came; an error once the device is gone.
    fn read(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>>;
    /// Sends one output report (report ID first).
    fn write(&mut self, report: &[u8]) -> io::Result<()>;
}

/// Finds and opens a controller; called on the HID thread, again after the
/// pad goes away. `None` while there is none.
pub type HidOpener = Box<dyn FnMut() -> Option<Box<dyn HidPad>> + Send>;

/// What the HID side does, from `H2_PAD_MUTE` and the speaker setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadHidConfig {
    /// The mute button toggles mute and the LED shows it.
    pub mute: bool,
}

impl PadHidConfig {
    /// `H2_PAD_MUTE`: `1` (the default when unset) or `0`. The mute button
    /// only does anything once a DualSense is found, so on is harmless
    /// without one.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mute = match var("H2_PAD_MUTE").as_deref().map(str::trim) {
            None | Some("" | "1") => true,
            Some("0") => false,
            Some(v) => return Err(format!("H2_PAD_MUTE: expected 0 or 1, got {v:?}")),
        };
        Ok(Self { mute })
    }
}

/// The reports one controller needs, as its state changes.
#[derive(Debug)]
pub struct PadLink {
    transport: Transport,
    mute_enabled: bool,
    mic: MicMute,
    speaker: bool,
    seq: u8,
}

impl PadLink {
    /// A link for a device just opened. `muted` carries the mute state over
    /// from a previous connection; `speaker` is whether the controller
    /// output is playing now.
    pub fn new(transport: Transport, config: PadHidConfig, muted: bool, speaker: bool) -> Self {
        Self {
            transport,
            mute_enabled: config.mute,
            mic: MicMute::new(muted),
            speaker,
            seq: 0,
        }
    }

    pub fn muted(&self) -> bool {
        self.mic.muted()
    }

    pub fn speaker(&self) -> bool {
        self.speaker
    }

    fn report(&mut self, req: OutputRequest) -> Option<Vec<u8>> {
        if req == OutputRequest::default() {
            return None;
        }
        let r = dualsense::output_report(self.transport, req, self.seq);
        self.seq = self.seq.wrapping_add(1);
        Some(r)
    }

    fn led(&self) -> Option<dualsense::MuteLed> {
        self.mute_enabled.then(|| self.mic.led())
    }

    /// What to send right after opening: the speaker path if the output is
    /// playing, and the LED for the current mute state if mute is on.
    pub fn on_open(&mut self) -> Option<Vec<u8>> {
        let req = OutputRequest {
            mute_led: self.led(),
            speaker: self.speaker.then_some(true),
        };
        self.report(req)
    }

    /// Feeds one input report; returns the LED update a press needs.
    pub fn on_input(&mut self, report: &[u8]) -> Option<Vec<u8>> {
        if !self.mute_enabled {
            return None;
        }
        let pressed = dualsense::mute_pressed(self.transport, report)?;
        if !self.mic.update(pressed) {
            return None;
        }
        let req = OutputRequest {
            mute_led: self.led(),
            speaker: None,
        };
        self.report(req)
    }

    /// The mute button turned on (`true`) or off during the run. On: the
    /// LED shows the mute state from now. Off: the microphone is unmuted
    /// and the LED put out, and presses are ignored. Nothing if it is
    /// already so.
    pub fn set_mute_enabled(&mut self, on: bool) -> Option<Vec<u8>> {
        if on == self.mute_enabled {
            return None;
        }
        self.mute_enabled = on;
        if !on {
            self.mic = MicMute::new(false);
            return self.report(OutputRequest {
                mute_led: Some(dualsense::MuteLed::Off),
                speaker: None,
            });
        }
        self.report(OutputRequest {
            mute_led: self.led(),
            speaker: None,
        })
    }

    /// The controller output started (`true`) or stopped: the speaker path
    /// on, or back to the headphone jack. Nothing if it is already so.
    pub fn set_speaker(&mut self, on: bool) -> Option<Vec<u8>> {
        if on == self.speaker {
            return None;
        }
        self.speaker = on;
        self.report(OutputRequest {
            mute_led: None,
            speaker: Some(on),
        })
    }
}

enum Cmd {
    Speaker(bool),
    Mute(bool),
}

/// What the HID thread has seen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PadHidStatus {
    pub connected: bool,
    pub muted: bool,
    /// Output reports written.
    pub reports: u64,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    connected: AtomicBool,
    muted: AtomicBool,
    reports: AtomicU64,
}

/// Tells a running HID thread that the controller output went live or
/// stopped (`dsound::SideOutput::on_live`). Cheap to clone.
#[derive(Clone)]
pub struct SpeakerSwitch(Sender<Cmd>);

impl SpeakerSwitch {
    pub fn set(&self, on: bool) {
        let _ = self.0.send(Cmd::Speaker(on));
    }
}

/// Turns the mute button on or off on a running HID thread (the Video
/// screen's Mute Button row). Cheap to clone.
#[derive(Clone)]
pub struct MuteSwitch(Sender<Cmd>);

impl MuteSwitch {
    pub fn set(&self, on: bool) {
        let _ = self.0.send(Cmd::Mute(on));
    }
}

/// The running HID thread. Dropping it puts the speaker path back (if it
/// was on and the pad is there) and joins.
pub struct PadHid {
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl PadHid {
    pub fn speaker_switch(&self) -> SpeakerSwitch {
        SpeakerSwitch(self.tx.clone())
    }

    pub fn mute_switch(&self) -> MuteSwitch {
        MuteSwitch(self.tx.clone())
    }

    pub fn status(&self) -> PadHidStatus {
        let s = &self.shared;
        PadHidStatus {
            connected: s.connected.load(Ordering::Acquire),
            muted: s.muted.load(Ordering::Acquire),
            reports: s.reports.load(Ordering::Acquire),
        }
    }
}

impl Drop for PadHid {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// How often the thread looks for a pad and polls for input.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Between attempts to find a pad.
    pub retry: Duration,
    /// Longest wait for an input report before commands are looked at.
    pub poll: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            retry: Duration::from_secs(1),
            poll: Duration::from_millis(20),
        }
    }
}

/// Starts the HID thread: it opens a pad through `open` (retrying while
/// there is none), sends what [`PadLink`] says, and follows the mute button
/// and [`SpeakerSwitch`] until dropped.
pub fn spawn(open: HidOpener, config: PadHidConfig, timing: Timing) -> PadHid {
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared::default());
    let sh = Arc::clone(&shared);
    let join = std::thread::Builder::new()
        .name("pad-hid".into())
        .spawn(move || run(open, config, timing, rx, sh))
        .expect("spawning the pad HID thread");
    PadHid {
        tx,
        shared,
        join: Some(join),
    }
}

fn send(pad: &mut dyn HidPad, report: Option<Vec<u8>>, shared: &Shared) -> io::Result<()> {
    if let Some(r) = report {
        pad.write(&r)?;
        shared.reports.fetch_add(1, Ordering::AcqRel);
    }
    Ok(())
}

fn run(
    mut open: HidOpener,
    mut config: PadHidConfig,
    timing: Timing,
    rx: Receiver<Cmd>,
    shared: Arc<Shared>,
) {
    let mut speaker = false;
    let mut muted = false;
    let mut last_try: Option<Instant> = None;
    // With no pad, a command only changes what the next one starts with.
    fn note(cmd: Cmd, config: &mut PadHidConfig, speaker: &mut bool, muted: &mut bool) {
        match cmd {
            Cmd::Speaker(on) => *speaker = on,
            Cmd::Mute(on) => {
                config.mute = on;
                *muted &= on;
            }
        }
    }
    while !shared.stop.load(Ordering::Acquire) {
        // No pad: follow the commands and look again now and then.
        if last_try.is_some_and(|t| t.elapsed() < timing.retry) {
            match rx.recv_timeout(timing.poll) {
                Ok(cmd) => note(cmd, &mut config, &mut speaker, &mut muted),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {}
            }
            continue;
        }
        while let Ok(cmd) = rx.try_recv() {
            note(cmd, &mut config, &mut speaker, &mut muted);
        }
        last_try = Some(Instant::now());
        let Some(mut pad) = open() else {
            continue;
        };
        let mut link = PadLink::new(pad.transport(), config, muted, speaker);
        shared.connected.store(true, Ordering::Release);
        let r = link.on_open();
        let mut alive = send(pad.as_mut(), r, &shared).is_ok();
        while alive && !shared.stop.load(Ordering::Acquire) {
            while let Ok(cmd) = rx.try_recv() {
                let r = match cmd {
                    Cmd::Speaker(on) => link.set_speaker(on),
                    Cmd::Mute(on) => {
                        note(Cmd::Mute(on), &mut config, &mut speaker, &mut muted);
                        link.set_mute_enabled(on)
                    }
                };
                shared.muted.store(link.muted(), Ordering::Release);
                alive &= send(pad.as_mut(), r, &shared).is_ok();
            }
            match pad.read(timing.poll) {
                Ok(Some(report)) => {
                    let r = link.on_input(&report);
                    alive &= send(pad.as_mut(), r, &shared).is_ok();
                    shared.muted.store(link.muted(), Ordering::Release);
                }
                Ok(None) => {}
                Err(_) => alive = false,
            }
        }
        if alive {
            // Stopping: the jack again, as the firmware has it on its own.
            let r = link.set_speaker(false);
            let _ = send(pad.as_mut(), r, &shared);
        }
        speaker = link.speaker();
        muted = link.muted();
        shared.connected.store(false, Ordering::Release);
    }
}

/// The macOS IOKit opener: the first wired or Bluetooth DualSense, opened
/// without seizing it.
#[cfg(all(target_os = "macos", feature = "hid"))]
pub fn iokit_opener() -> HidOpener {
    Box::new(iokit::open)
}

/// This platform's opener ([`iokit_opener`] on macOS with the `hid`
/// feature), or `None` where raw HID is not built.
pub fn platform_opener() -> Option<HidOpener> {
    #[cfg(all(target_os = "macos", feature = "hid"))]
    return Some(iokit_opener());
    #[cfg(not(all(target_os = "macos", feature = "hid")))]
    None
}

#[cfg(all(target_os = "macos", feature = "hid"))]
#[path = "pad_hid_iokit.rs"]
mod iokit;

#[cfg(test)]
#[path = "pad_hid_tests.rs"]
mod tests;
