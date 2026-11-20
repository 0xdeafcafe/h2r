//! Steam Controllers over raw HID: a thread that owns them, and the
//! [`HostBackend`] the game's pads come from (`docs/input/steam-controller.md`).
//!
//! No platform framework reads a Steam Controller as a gamepad
//! (GameController.framework does not list it, and in lizard mode it is a
//! keyboard and a mouse), so this is a backend of its own. The thread
//! ([`spawn`]) opens every Valve controller interface, turns lizard mode off
//! while it holds one ([`steam::lizard_off`], kept off on the 2026
//! controller with [`steam::keepalive`]) and back on when it lets go or
//! stops, parses each input report ([`steam::Parser`]) into a standard pad
//! ([`trackpad::to_host`]) and the right trackpad's travel into look motion
//! ([`trackpad::TrackpadAim`], [`LookMotion`]), and every
//! [`haptics::FRAME`] sends the game's rumble and the routed sound to the
//! trackpad actuators ([`haptics::HapticMixer`]).
//!
//! [`Hub`] is that logic over [`HidEvent`]s, testable without a device; the
//! macOS IOKit half (`steam_pad_iokit.rs`) only delivers events and writes
//! reports.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::backend::HostBackend;
use crate::gamepad::Rumble;
use crate::haptics::{self, HapticMixer, Tone};
use crate::host::{HostPad, HostPadId, HostPadState};
use crate::look::LookMotion;
use crate::steam::{self, Event, HidWrite, Link, Model, Parser};
use crate::trackpad::{self, AimTuning, LeftPadMode, TrackpadAim};

/// The name every Steam Controller pad reports (`active::pad_kind` and the
/// icon sets key on it).
pub const PAD_NAME: &str = "Steam Controller";

/// One HID interface the host found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HidDevice {
    /// Stable while the interface stays.
    pub key: u64,
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u32,
    pub usage: u32,
}

/// What the host saw since the last wait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HidEvent {
    Added(HidDevice),
    Removed(u64),
    /// An input report, report ID first if the device numbers its reports.
    Report(u64, Vec<u8>),
}

/// The host's half: every Valve HID interface, opened without seizing it.
pub trait SteamHid {
    /// Events, waiting at most `timeout` for the first. An error ends this
    /// host (the thread opens a new one later).
    fn wait(&mut self, timeout: Duration) -> io::Result<Vec<HidEvent>>;
    /// Wake an in-progress or upcoming wait from another thread. Hosts
    /// without a waker bound their wait by [`Timing::poll`] at shutdown.
    fn wake_handle(&self) -> Option<Box<dyn Fn() + Send + Sync>> {
        None
    }
    /// Sends one report to the interface `key`.
    fn write(&mut self, key: u64, report: &HidWrite) -> io::Result<()>;
}

/// Opens the host side; called on the thread, again after a failure.
pub type SteamOpener = Box<dyn FnMut() -> Option<Box<dyn SteamHid>> + Send>;

/// The settings the thread and backend follow while running: the Steam
/// Controller rows (`steam_pad.*` in the settings store). Cheap to clone.
#[derive(Clone, Debug)]
pub struct SteamTuning(Arc<TuningCells>);

#[derive(Debug)]
struct TuningCells {
    stick_mode: AtomicBool,
    sensitivity: AtomicU8,
    speaker: AtomicBool,
}

impl SteamTuning {
    /// `sensitivity` is the 1..=10 setting (5 is 1.0).
    pub fn new(left_pad: LeftPadMode, sensitivity: u8, speaker: bool) -> Self {
        Self(Arc::new(TuningCells {
            stick_mode: AtomicBool::new(left_pad == LeftPadMode::Stick),
            sensitivity: AtomicU8::new(sensitivity.clamp(1, 10)),
            speaker: AtomicBool::new(speaker),
        }))
    }

    pub fn left_pad(&self) -> LeftPadMode {
        if self.0.stick_mode.load(Ordering::Relaxed) {
            LeftPadMode::Stick
        } else {
            LeftPadMode::Dpad
        }
    }

    pub fn set_left_pad(&self, mode: LeftPadMode) {
        self.0
            .stick_mode
            .store(mode == LeftPadMode::Stick, Ordering::Relaxed);
    }

    pub fn sensitivity(&self) -> u8 {
        self.0.sensitivity.load(Ordering::Relaxed)
    }

    pub fn set_sensitivity(&self, v: u8) {
        self.0.sensitivity.store(v.clamp(1, 10), Ordering::Relaxed);
    }

    /// The haptic speaker plays routed sounds.
    pub fn speaker(&self) -> bool {
        self.0.speaker.load(Ordering::Relaxed)
    }

    pub fn set_speaker(&self, on: bool) {
        self.0.speaker.store(on, Ordering::Relaxed);
    }

    fn aim(&self) -> AimTuning {
        AimTuning {
            sensitivity: trackpad::sensitivity_multiplier(self.sensitivity()),
            ..AimTuning::default()
        }
    }
}

impl Default for SteamTuning {
    fn default() -> Self {
        Self::new(LeftPadMode::Dpad, 5, true)
    }
}

/// A connected controller as the backend sees it.
#[derive(Clone, Debug)]
struct Slot {
    key: u64,
    id: HostPadId,
    host: HostPadState,
    look: LookMotion,
    rumble: Rumble,
}

/// What the thread and the backend share.
#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    wake: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    slots: Mutex<Vec<Slot>>,
    /// Frames of routed sound waiting for the actuators.
    tones: Mutex<VecDeque<Option<Tone>>>,
    reports: AtomicU64,
}

/// Tones queued beyond this are dropped from the front (the actuators run
/// late rather than ever further behind).
pub const MAX_TONES: usize = 4;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn push_tone(&self, t: Option<Tone>) {
        let mut q = lock(&self.tones);
        q.push_back(t);
        while q.len() > MAX_TONES {
            q.pop_front();
        }
    }

    fn next_tone(&self) -> Option<Tone> {
        lock(&self.tones).pop_front().flatten()
    }
}

/// One Valve controller interface.
#[derive(Debug)]
struct Device {
    model: Model,
    link: Link,
    parser: Parser,
    /// A controller is on this interface (always for wired and Bluetooth;
    /// for a receiver slot, from its connect event or first state).
    connected: bool,
    aim: TrackpadAim,
    mixer: HapticMixer,
    keepalive_at: Option<Instant>,
}

/// The Steam Controller logic over host events: which interfaces hold a
/// controller, what to send them and the pads they give.
#[derive(Debug, Default)]
pub struct Hub {
    devices: BTreeMap<u64, Device>,
    next_id: u64,
}

impl Hub {
    fn writes(d: &Device, key: u64, ws: Vec<HidWrite>) -> Vec<(u64, HidWrite)> {
        ws.into_iter()
            .flat_map(|w| steam::on_link(d.model, d.link, w))
            .map(|w| (key, w))
            .collect()
    }

    fn connect(&mut self, key: u64, now: Instant, shared: &Shared) -> Vec<(u64, HidWrite)> {
        let Some(d) = self.devices.get_mut(&key) else {
            return Vec::new();
        };
        if d.connected {
            return Vec::new();
        }
        d.connected = true;
        d.keepalive_at = Some(now + steam::KEEPALIVE);
        d.aim = TrackpadAim::default();
        d.mixer = HapticMixer::new(d.model);
        self.next_id += 1;
        lock(&shared.slots).push(Slot {
            key,
            id: HostPadId(self.next_id),
            host: HostPadState::default(),
            look: LookMotion::default(),
            rumble: Rumble::default(),
        });
        let d = &self.devices[&key];
        Self::writes(d, key, steam::lizard_off(d.model))
    }

    fn disconnect(&mut self, key: u64, shared: &Shared) {
        if let Some(d) = self.devices.get_mut(&key) {
            d.connected = false;
        }
        lock(&shared.slots).retain(|s| s.key != key);
    }

    /// Handles one host event; returns the reports to send.
    fn event(
        &mut self,
        ev: HidEvent,
        now: Instant,
        shared: &Shared,
        tuning: &SteamTuning,
    ) -> Vec<(u64, HidWrite)> {
        match ev {
            HidEvent::Added(info) => {
                let Some((model, link)) = steam::identify(info.vid, info.pid) else {
                    return Vec::new();
                };
                if !steam::is_controller_interface(info.usage_page, info.usage)
                    || self.devices.contains_key(&info.key)
                {
                    return Vec::new();
                }
                self.devices.insert(
                    info.key,
                    Device {
                        model,
                        link,
                        parser: Parser::new(model, link),
                        connected: false,
                        aim: TrackpadAim::default(),
                        mixer: HapticMixer::new(model),
                        keepalive_at: None,
                    },
                );
                // A receiver slot waits for its controller.
                if link == Link::Dongle {
                    return Vec::new();
                }
                self.connect(info.key, now, shared)
            }
            HidEvent::Removed(key) => {
                self.disconnect(key, shared);
                self.devices.remove(&key);
                Vec::new()
            }
            HidEvent::Report(key, bytes) => {
                let Some(d) = self.devices.get_mut(&key) else {
                    return Vec::new();
                };
                match d.parser.feed(&bytes) {
                    Some(Event::Connected) => self.connect(key, now, shared),
                    Some(Event::Disconnected) => {
                        self.disconnect(key, shared);
                        Vec::new()
                    }
                    Some(Event::State(s)) => {
                        let out = self.connect(key, now, shared);
                        let d = self.devices.get_mut(&key).expect("present");
                        let host = trackpad::to_host(d.model, &s, tuning.left_pad());
                        let (dx, dy) = d.aim.update(s.right_pad, now, &tuning.aim());
                        if let Some(slot) = lock(&shared.slots).iter_mut().find(|s| s.key == key) {
                            slot.host = host;
                            if dx != 0.0 || dy != 0.0 {
                                slot.look.add(now, dx, dy);
                            }
                        }
                        out
                    }
                    None => Vec::new(),
                }
            }
        }
    }

    /// One haptic frame: the keepalives that are due, then each
    /// controller's rumble and the routed sound's `tone` (when the speaker
    /// is on).
    fn frame(
        &mut self,
        now: Instant,
        tone: Option<Tone>,
        shared: &Shared,
        tuning: &SteamTuning,
    ) -> Vec<(u64, HidWrite)> {
        let tone = tone.filter(|_| tuning.speaker());
        let rumbles: BTreeMap<u64, Rumble> = lock(&shared.slots)
            .iter()
            .map(|s| (s.key, s.rumble))
            .collect();
        let mut out = Vec::new();
        for (&key, d) in self.devices.iter_mut().filter(|(_, d)| d.connected) {
            let mut ws = Vec::new();
            if d.keepalive_at.is_some_and(|t| now >= t) {
                d.keepalive_at = Some(now + steam::KEEPALIVE);
                ws.extend(steam::keepalive(d.model));
            }
            let rumble = rumbles.get(&key).copied().unwrap_or_default();
            ws.extend(d.mixer.frame(rumble, tone));
            out.extend(Self::writes(d, key, ws));
        }
        out
    }

    /// Letting go: motors stopped and lizard mode back on every controller.
    fn release(&mut self, shared: &Shared) -> Vec<(u64, HidWrite)> {
        let mut out = Vec::new();
        for (&key, d) in self.devices.iter().filter(|(_, d)| d.connected) {
            let mut ws = Vec::new();
            if d.model == Model::Triton {
                ws.push(steam::triton_rumble(0, 0));
            }
            ws.extend(steam::lizard_on(d.model));
            out.extend(Self::writes(d, key, ws));
        }
        let keys: Vec<u64> = self.devices.keys().copied().collect();
        for key in keys {
            self.disconnect(key, shared);
        }
        self.devices.clear();
        out
    }
}

/// How often the thread looks for the host side and waits for reports.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub retry: Duration,
    pub poll: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            retry: Duration::from_secs(1),
            poll: Duration::from_millis(4),
        }
    }
}

/// The running Steam Controller thread. Clones share it; the last one
/// dropped gives lizard mode back and joins.
#[derive(Clone)]
pub struct SteamPads {
    shared: Arc<Shared>,
    tuning: SteamTuning,
    _thread: Arc<Joiner>,
}

struct Joiner {
    shared: Arc<Shared>,
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Drop for Joiner {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(wake) = lock(&self.shared.wake).take() {
            wake();
        }
        if let Some(j) = lock(&self.join).take() {
            j.thread().unpark(); // An open retry can be asleep without a HID host.
            let _ = j.join();
        }
    }
}

impl std::fmt::Debug for SteamPads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteamPads")
            .field("connected", &self.connected())
            .field("tuning", &self.tuning)
            .finish()
    }
}

impl SteamPads {
    /// A backend over these controllers, for `InputManager::add_backend`
    /// (or a combined backend).
    pub fn backend(&self) -> SteamBackend {
        SteamBackend {
            shared: Arc::clone(&self.shared),
            _thread: Arc::clone(&self._thread),
        }
    }

    pub fn tuning(&self) -> &SteamTuning {
        &self.tuning
    }

    /// Controllers connected now.
    pub fn connected(&self) -> usize {
        lock(&self.shared.slots).len()
    }

    /// Whether routed sound would play: a controller is connected and the
    /// haptic speaker is on.
    pub fn speaker_live(&self) -> bool {
        self.tuning.speaker() && self.connected() > 0
    }

    /// One [`haptics::FRAME`] of routed sound (or `None`, silence) for the
    /// actuators; called by the audio side as it renders.
    pub fn push_tone(&self, tone: Option<Tone>) {
        self.shared.push_tone(tone);
    }

    /// Reports written so far.
    pub fn reports(&self) -> u64 {
        self.shared.reports.load(Ordering::Acquire)
    }
}

/// Starts the thread over `open`.
pub fn spawn(open: SteamOpener, tuning: SteamTuning, timing: Timing) -> SteamPads {
    let shared = Arc::new(Shared::default());
    let sh = Arc::clone(&shared);
    let tu = tuning.clone();
    let join = std::thread::Builder::new()
        .name("steam-pad".into())
        .spawn(move || run(open, tu, timing, sh))
        .expect("spawning the Steam Controller thread");
    SteamPads {
        shared: Arc::clone(&shared),
        tuning,
        _thread: Arc::new(Joiner {
            shared,
            join: Mutex::new(Some(join)),
        }),
    }
}

fn send(hid: &mut dyn SteamHid, writes: Vec<(u64, HidWrite)>, shared: &Shared) {
    for (key, w) in writes {
        // A failed write is the device going away; its removal follows.
        if hid.write(key, &w).is_ok() {
            shared.reports.fetch_add(1, Ordering::AcqRel);
        }
    }
}

fn run(mut open: SteamOpener, tuning: SteamTuning, timing: Timing, shared: Arc<Shared>) {
    let mut hub = Hub::default();
    let mut hid: Option<Box<dyn SteamHid>> = None;
    let mut last_try: Option<Instant> = None;
    let mut next_frame = Instant::now();
    let mut interruptible = false;
    while !shared.stop.load(Ordering::Acquire) {
        let Some(h) = hid.as_mut() else {
            let retry_in = last_try.map(|t| timing.retry.saturating_sub(t.elapsed())).unwrap_or_default();
            if !retry_in.is_zero() {
                std::thread::park_timeout(retry_in);
                continue;
            }
            last_try = Some(Instant::now());
            hid = open();
            if let Some(h) = &hid {
                let wake = h.wake_handle();
                interruptible = wake.is_some();
                *lock(&shared.wake) = wake;
            }
            continue;
        };
        let connected = hub.devices.values().any(|device| device.connected);
        let until_frame = if connected {
            next_frame.saturating_duration_since(Instant::now())
        } else {
            // Only a device event or shutdown needs to wake the IOKit loop.
            Duration::from_secs(24 * 60 * 60)
        };
        let timeout = if interruptible { until_frame } else { until_frame.min(timing.poll) };
        let events = match h.wait(timeout) {
            Ok(e) => e,
            Err(_) => {
                let _ = hub.release(&shared);
                *lock(&shared.wake) = None;
                hid = None;
                continue;
            }
        };
        let now = Instant::now();
        let mut writes = Vec::new();
        for ev in events {
            writes.extend(hub.event(ev, now, &shared, &tuning));
        }
        if now >= next_frame && hub.devices.values().any(|device| device.connected) {
            next_frame += haptics::FRAME;
            if next_frame <= now {
                next_frame = now + haptics::FRAME;
            }
            writes.extend(hub.frame(now, shared.next_tone(), &shared, &tuning));
        }
        send(h.as_mut(), writes, &shared);
    }
    if let Some(mut h) = hid {
        let writes = hub.release(&shared);
        send(h.as_mut(), writes, &shared);
    }
}

/// The Steam Controllers as host pads.
pub struct SteamBackend {
    shared: Arc<Shared>,
    _thread: Arc<Joiner>,
}

impl HostBackend for SteamBackend {
    fn name(&self) -> &str {
        "steam-controller"
    }

    /// Each connected controller, its right-trackpad motion on the right
    /// stick as mouse look puts it there ([`LookMotion::stick`], at
    /// [`trackpad::LOOK_K`]), scaled back to the unit circle.
    fn poll(&mut self) -> Vec<HostPad> {
        let now = Instant::now();
        lock(&self.shared.slots)
            .iter_mut()
            .map(|s| {
                let mut state = s.host;
                let (lx, ly) = s.look.stick(now, trackpad::LOOK_K, false);
                let r = &mut state.right_stick;
                r.0 = (f64::from(r.0) + lx) as f32;
                r.1 = (f64::from(r.1) + ly) as f32;
                let m = r.0.hypot(r.1);
                if m > 1.0 {
                    r.0 /= m;
                    r.1 /= m;
                }
                HostPad {
                    id: s.id,
                    name: PAD_NAME.into(),
                    state,
                    physical: true,
                    supports_rumble: true,
                }
            })
            .collect()
    }

    fn state_read(&mut self, id: HostPadId) {
        let now = Instant::now();
        if let Some(s) = lock(&self.shared.slots).iter_mut().find(|s| s.id == id) {
            s.look.read(now);
        }
    }

    fn set_rumble(&mut self, id: HostPadId, rumble: Rumble) {
        if let Some(s) = lock(&self.shared.slots).iter_mut().find(|s| s.id == id) {
            s.rumble = rumble;
        }
    }
}

/// The macOS IOKit host: every Valve controller interface, unseized.
#[cfg(all(target_os = "macos", feature = "hid"))]
pub fn iokit_opener() -> SteamOpener {
    Box::new(iokit::open)
}

/// This platform's host ([`iokit_opener`] on macOS with the `hid` feature),
/// or `None` where raw HID is not built.
pub fn platform_opener() -> Option<SteamOpener> {
    #[cfg(all(target_os = "macos", feature = "hid"))]
    return Some(iokit_opener());
    #[cfg(not(all(target_os = "macos", feature = "hid")))]
    None
}

#[cfg(all(target_os = "macos", feature = "hid"))]
#[path = "steam_pad_iokit.rs"]
mod iokit;

#[cfg(test)]
#[path = "steam_pad_tests.rs"]
mod tests;
