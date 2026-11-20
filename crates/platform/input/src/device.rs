//! The four-port Xbox device model.
//!
//! [`InputManager`] is what the replacement for `XInitDevices`, `XGetDevices`,
//! `XGetDeviceChanges`, `XInputOpen/Close/Poll/GetState/SetState` calls into.
//! The host side is driven by [`InputManager::sync`], called once per host
//! frame (or whenever the emulated USB stack would have polled).
//!
//! Semantics:
//!
//! * Ports 0..=3. A host pad takes the lowest free port when it first appears
//!   and keeps it until it disappears. Backends added first win free ports
//!   first.
//! * Insertion creates a new device instance (new generation, packet number
//!   0). Handles opened on a previous instance of the port go stale and return
//!   `ERROR_DEVICE_NOT_CONNECTED`, like a handle to an unplugged pad.
//! * `dwPacketNumber` increments only when the reported `XINPUT_GAMEPAD`
//!   differs from the last published one (Cxbx-Reloaded `Xapi.cpp` increments
//!   it when new input is read; a real pad's report only changes on change).
//! * With auto-poll (the default `fAutoPoll = 1`) state is published on every
//!   `sync`. With auto-poll off, state is published only by [`poll`](InputManager::poll),
//!   so `get_state` returns the last polled snapshot.
//! * `device_changes` reports every insertion and removal since the last call,
//!   accumulated as they happen in `sync` (insert then remove between reads
//!   shows up in both masks).
//! * Rumble: every `set_state` is its own request, identified by the guest
//!   address of its `XINPUT_FEEDBACK`. `set_state` returns `ERROR_IO_PENDING`;
//!   the next `sync` completes each pending request independently
//!   (`ERROR_SUCCESS`) and the runtime drains them with `take_completions`,
//!   writing `dwStatus` at the address and signalling `hEvent`. If the pad is
//!   unplugged first, pending requests complete with
//!   `ERROR_DEVICE_NOT_CONNECTED`. Requests are applied in order; the last
//!   rumble value wins on the backend.
//! * A [`PadGate`] ([`InputManager::set_gate`]) sees every pad's host state
//!   after the backends report it and before the ports convert it, and may
//!   change it: the debug menu reads its touchpad gestures there and hands
//!   the game neutral pads while it is open (docs/runtime/debug-menu.md).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::backend::HostBackend;
use crate::backend::keyboard::KeyboardMouse;
use crate::bindings::Bindings;
use crate::gamepad::{
    Capabilities, ERROR_DEVICE_NOT_CONNECTED, ERROR_IO_PENDING, ERROR_SUCCESS, Gamepad,
    PollingParameters, Rumble, State,
};
use crate::host::{ConfigError, ConvertConfig, HostPad, HostPadId};
use crate::lightbar::{Lightbars, Rgb};
use crate::report::{HostReportHandle, PadInfo};

/// One pad as a [`PadGate`] sees it: which backend reported it, and its
/// state, which the gate may change before the game sees it.
#[derive(Clone, Debug)]
pub struct GatedPad {
    /// The backend's index (in [`InputManager::add_backend`] order) and the
    /// pad's id within it: stable while the pad stays connected.
    pub key: (usize, HostPadId),
    pub pad: HostPad,
}

/// Sees every connected pad at each [`InputManager::sync`], before the
/// ports do, in backend order. Whatever it leaves in `pads` is what the
/// game gets; connection (and so the ports) is not its business.
pub trait PadGate: Send {
    fn filter(&mut self, pads: &mut [GatedPad]);
}

pub const PORT_COUNT: usize = 4;

/// A valid port number, 0..=3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PortIndex(u8);

impl PortIndex {
    pub fn new(p: usize) -> Option<Self> {
        (p < PORT_COUNT).then_some(PortIndex(p as u8))
    }
    pub fn get(self) -> usize {
        self.0 as usize
    }
    /// `XDEVICE_PORTn_MASK`.
    pub fn mask(self) -> u32 {
        1 << self.0
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InputError {
    #[error("device not connected")]
    NotConnected,
    #[error("device does not support rumble")]
    NotSupported,
}

impl InputError {
    /// The Win32 code the guest should see.
    pub fn win32(&self) -> u32 {
        match self {
            Self::NotConnected => ERROR_DEVICE_NOT_CONNECTED,
            Self::NotSupported => 50,
        }
    }
}

/// Result of `XInputOpen`: valid while the same device instance stays plugged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle {
    port: PortIndex,
    generation: u32,
}

impl Handle {
    pub fn port(&self) -> PortIndex {
        self.port
    }
}

struct Device {
    host: (usize, HostPadId),
    physical: bool,
    supports_rumble: bool,
    latest: Gamepad,
    published: State,
    polling: PollingParameters,
    rumble_sent: Rumble,
    lightbar_sent: Option<Rgb>,
    pending: Vec<FeedbackRequest>,
    /// `get_state` returned this device's state since the last `sync`.
    read: AtomicBool,
}

/// One queued `XInputSetState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedbackRequest {
    /// Guest address of the `XINPUT_FEEDBACK` (identifies the request).
    pub addr: u32,
    /// `Header.hEvent` to signal on completion (0 = none).
    pub h_event: u32,
    pub rumble: Rumble,
}

/// A finished feedback request: write `status` to `dwStatus` at `addr` and
/// signal `h_event`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub addr: u32,
    pub h_event: u32,
    pub status: u32,
}

impl Device {
    fn complete_all(&mut self, status: u32, out: &mut Vec<Completion>) {
        out.extend(self.pending.drain(..).map(|r| Completion {
            addr: r.addr,
            h_event: r.h_event,
            status,
        }));
    }
}

#[derive(Default)]
struct Port {
    device: Option<Device>,
    /// Bumped on every insertion.
    generation: u32,
}

pub struct InputManager {
    backends: Vec<Box<dyn HostBackend>>,
    ports: [Port; PORT_COUNT],
    convert: ConvertConfig,
    /// Insertions/removals since the last `device_changes`.
    changes_ins: u32,
    changes_rem: u32,
    completions: Vec<Completion>,
    gate: Option<Box<dyn PadGate>>,
    /// The host pads as of the last sync ([`Self::host_report`]).
    report: HostReportHandle,
    lightbars: Lightbars,
}

impl InputManager {
    /// Fails if the deadzone settings are not finite values in `[0, 1)`.
    pub fn new(convert: ConvertConfig) -> Result<Self, ConfigError> {
        convert.validate()?;
        Ok(InputManager {
            backends: Vec::new(),
            ports: Default::default(),
            convert,
            changes_ins: 0,
            changes_rem: 0,
            completions: Vec::new(),
            gate: None,
            report: HostReportHandle::default(),
            lightbars: Lightbars::default(),
        })
    }

    /// The default host input: the keyboard+mouse pad as the first backend,
    /// so it takes port 0 and is connected from the first `sync`, with
    /// rumble accepted like a standard controller. Keep the returned handle
    /// in the platform layer and feed it key/mouse events; add physical pads
    /// after it. Deadzones come from `bindings.convert`.
    pub fn with_keyboard_pad(bindings: Bindings) -> Result<(Self, KeyboardMouse), ConfigError> {
        let mut manager = Self::new(bindings.convert)?;
        let pad = KeyboardMouse::new(bindings);
        pad.set_rumble_supported(true);
        manager.add_backend(Box::new(pad.clone()));
        Ok((manager, pad))
    }

    pub fn add_backend(&mut self, b: Box<dyn HostBackend>) {
        self.backends.push(b);
    }

    /// Puts `gate` between the backends and the ports (replacing any).
    pub fn set_gate(&mut self, gate: Box<dyn PadGate>) {
        self.gate = Some(gate);
    }

    /// The host pads as the backends and the gate saw them at each sync
    /// ([`crate::report`]): what `h2 agent hostpads` answers.
    pub fn host_report(&self) -> HostReportHandle {
        self.report.clone()
    }

    pub fn lightbars(&self) -> Lightbars {
        self.lightbars.clone()
    }

    /// Pull host state and update the ports: removals, insertions, auto-poll
    /// publishing and rumble forwarding.
    pub fn sync(&mut self) {
        // Tell backends which states the guest has read, before they report
        // new ones: a consumable input (mouse motion) is spent only once seen.
        for port in &self.ports {
            if let Some(d) = &port.device
                && d.read.swap(false, Ordering::Relaxed)
            {
                self.backends[d.host.0].state_read(d.host.1);
            }
        }
        let mut seen: HashMap<(usize, HostPadId), crate::host::HostPad> = HashMap::new();
        let mut order = Vec::new();
        for (bi, b) in self.backends.iter_mut().enumerate() {
            for pad in b.poll() {
                let key = (bi, pad.id);
                order.push(key);
                seen.insert(key, pad);
            }
        }
        // What the gate is handed, for the report.
        let mut infos: Vec<PadInfo> = order
            .iter()
            .map(|k| {
                let p = &seen[k];
                PadInfo {
                    backend: self.backends[k.0].name().to_owned(),
                    key: (k.0, k.1.0),
                    name: p.name.clone(),
                    physical: p.physical,
                    port: None,
                    state: p.state,
                }
            })
            .collect();
        if let Some(gate) = &mut self.gate {
            let mut gated: Vec<GatedPad> = order
                .iter()
                .filter_map(|k| seen.remove(k).map(|pad| GatedPad { key: *k, pad }))
                .collect();
            gate.filter(&mut gated);
            seen.extend(gated.into_iter().map(|g| (g.key, g.pad)));
        }

        // Removals.
        for (i, port) in self.ports.iter_mut().enumerate() {
            if port
                .device
                .as_ref()
                .is_some_and(|d| !seen.contains_key(&d.host))
            {
                let mut d = port.device.take().expect("checked");
                if d.lightbar_sent.is_some() {
                    self.backends[d.host.0].set_lightbar(d.host.1, None);
                }
                d.complete_all(ERROR_DEVICE_NOT_CONNECTED, &mut self.completions);
                self.changes_rem |= 1 << i;
            }
        }
        // Insertions, in backend order, each on the port its backend asks
        // for or the lowest free one.
        for key in &order {
            if self
                .ports
                .iter()
                .any(|p| p.device.as_ref().is_some_and(|d| d.host == *key))
            {
                continue;
            }
            // The port the backend asks for when it is free, else the lowest
            // free one.
            let free = |i: usize| self.ports.get(i).is_some_and(|p| p.device.is_none());
            let wanted = self.backends[key.0]
                .preferred_port(key.1)
                .filter(|&i| free(i));
            let Some(i) = wanted.or_else(|| self.ports.iter().position(|p| p.device.is_none()))
            else {
                break; // all four ports taken
            };
            let port = &mut self.ports[i];
            self.changes_ins |= 1 << i;
            let pad = &seen[key];
            port.generation = port.generation.wrapping_add(1);
            port.device = Some(Device {
                host: *key,
                physical: pad.physical,
                supports_rumble: pad.supports_rumble,
                latest: Gamepad::default(),
                published: State::default(),
                polling: PollingParameters::default(),
                rumble_sent: Rumble::default(),
                lightbar_sent: None,
                pending: Vec::new(),
                read: AtomicBool::new(false),
            });
        }
        // Update.
        let cfg = self.convert;
        for port in &mut self.ports {
            let Some(d) = &mut port.device else { continue };
            let pad = &seen[&d.host];
            d.latest = pad.state.to_gamepad(&cfg, d.physical);
            if d.polling.auto_poll {
                publish(d);
            }
            if let Some(last) = d.pending.last().map(|r| r.rumble)
                && last != d.rumble_sent
            {
                if d.supports_rumble {
                    self.backends[d.host.0].set_rumble(d.host.1, last);
                }
                d.rumble_sent = last;
            }
            d.complete_all(ERROR_SUCCESS, &mut self.completions);
        }
        // Route output by the same port assignment the guest's input uses.
        let lightbars = self.lightbars.state().desired();
        for (i, port) in self.ports.iter_mut().enumerate() {
            if let Some(d) = &mut port.device {
                let backend = &mut self.backends[d.host.0];
                backend.placed(d.host.1, i);
                let rgb = lightbars[i];
                if d.lightbar_sent != rgb {
                    backend.set_lightbar(d.host.1, rgb);
                    d.lightbar_sent = rgb;
                }
                if let Some(info) = infos.iter_mut().find(|p| p.key == (d.host.0, d.host.1.0)) {
                    info.port = Some(i);
                }
            }
        }
        let devices = self.backends.iter().flat_map(|b| b.devices()).collect();
        self.report.set(devices, infos);
    }

    /// Return each pad to its original colour before ending the game.
    pub fn restore_lightbars(&mut self) {
        for port in &mut self.ports {
            if let Some(d) = &mut port.device
                && d.lightbar_sent.take().is_some()
            {
                self.backends[d.host.0].set_lightbar(d.host.1, None);
            }
        }
    }

    /// Device generation and output capability for each occupied guest port.
    /// A new generation distinguishes a replacement on a still-occupied port.
    pub fn port_connections(&self) -> [Option<(u32, bool)>; PORT_COUNT] {
        std::array::from_fn(|i| {
            let port = &self.ports[i];
            port.device.as_ref().map(|d| (port.generation, d.supports_rumble))
        })
    }

    /// `XGetDevices(XDEVICE_TYPE_GAMEPAD)`: bitmask of connected ports.
    pub fn connected_mask(&self) -> u32 {
        (0..PORT_COUNT)
            .filter(|&i| self.ports[i].device.is_some())
            .fold(0, |m, i| m | 1 << i)
    }

    /// Raw host `(insertions, removals)` since the previous drain. XAPI applies
    /// the retail Cur/Prev/Chg formulas (guest 0x004098c5) to these edges; an
    /// insert/remove with Prev=Cur=0 must not become a guest insertion.
    pub fn device_changes(&mut self) -> (u32, u32) {
        (
            std::mem::take(&mut self.changes_ins),
            std::mem::take(&mut self.changes_rem),
        )
    }

    /// Drain finished feedback requests.
    pub fn take_completions(&mut self) -> Vec<Completion> {
        std::mem::take(&mut self.completions)
    }

    /// `XInputOpen`.
    pub fn open(&self, port: PortIndex) -> Result<Handle, InputError> {
        let p = &self.ports[port.get()];
        p.device
            .as_ref()
            .map(|_| Handle {
                port,
                generation: p.generation,
            })
            .ok_or(InputError::NotConnected)
    }

    /// `XInputClose`. Handles carry no resources here; kept for symmetry.
    pub fn close(&self, _h: Handle) {}

    fn device(&self, h: Handle) -> Result<&Device, InputError> {
        let p = &self.ports[h.port.get()];
        match &p.device {
            Some(d) if p.generation == h.generation => Ok(d),
            _ => Err(InputError::NotConnected),
        }
    }

    fn device_mut(&mut self, h: Handle) -> Result<&mut Device, InputError> {
        let p = &mut self.ports[h.port.get()];
        match &mut p.device {
            Some(d) if p.generation == h.generation => Ok(d),
            _ => Err(InputError::NotConnected),
        }
    }

    /// `XInputGetState`: the guest reads the published state. The next `sync`
    /// tells the device's backend that its state was seen
    /// ([`HostBackend::state_read`]), which spends pending mouse motion. Host
    /// bookkeeping that is not a guest read uses [`peek_state`](Self::peek_state).
    pub fn get_state(&self, h: Handle) -> Result<State, InputError> {
        self.device(h).map(|d| {
            d.read.store(true, Ordering::Relaxed);
            d.published
        })
    }

    /// The published state, without counting as a guest read.
    pub fn peek_state(&self, h: Handle) -> Result<State, InputError> {
        self.device(h).map(|d| d.published)
    }

    /// `XInputPoll`: publish the newest host state (matters when auto-poll is
    /// off; harmless otherwise).
    pub fn poll(&mut self, h: Handle) -> Result<(), InputError> {
        publish(self.device_mut(h)?);
        Ok(())
    }

    /// `XInputGetCapabilities`.
    pub fn capabilities(&self, handle: Handle) -> Result<Capabilities, InputError> {
        self.device(handle).map(|device| {
            let mut capabilities = Capabilities::standard();
            if device.supports_rumble {
                capabilities.output = Rumble {
                    left_motor: u16::MAX,
                    right_motor: u16::MAX,
                };
            }
            capabilities
        })
    }

    pub fn polling_parameters(&self, h: Handle) -> Result<PollingParameters, InputError> {
        self.device(h).map(|d| d.polling)
    }

    /// `XInputSetPollingParameters`.
    pub fn set_polling_parameters(
        &mut self,
        h: Handle,
        p: PollingParameters,
    ) -> Result<(), InputError> {
        self.device_mut(h)?.polling = p;
        Ok(())
    }

    /// `XInputSetState`: queue a rumble request on a capable backend, otherwise
    /// return NotSupported (Win32 50). Returns `ERROR_IO_PENDING` on submission
    /// (the value for the call and the initial `dwStatus`); the request later
    /// appears in [`take_completions`](Self::take_completions).
    pub fn set_state(
        &mut self,
        handle: Handle,
        request: FeedbackRequest,
    ) -> Result<u32, InputError> {
        let device = self.device_mut(handle)?;
        if !device.supports_rumble {
            return Err(InputError::NotSupported);
        }
        device.pending.push(request);
        Ok(ERROR_IO_PENDING)
    }

    /// What a machine snapshot keeps of the device on `port`
    /// (docs/runtime/snapshots.md): its published state, whose packet
    /// number the guest compares, and its polling parameters. `None` when
    /// the port is empty.
    pub fn port_snapshot(&self, port: PortIndex) -> Option<PortSnapshot> {
        self.ports[port.get()]
            .device
            .as_ref()
            .map(|d| PortSnapshot {
                published: d.published,
                polling: d.polling,
            })
    }

    /// Put a saved port's published state and polling parameters on the
    /// device now on `port`, so packet numbers continue from the saved
    /// run's. The host's own state is published over it at the next
    /// `sync` or `poll` (held keys are not carried). Returns false when the
    /// port is empty: the loaded handle then reads as unplugged.
    pub fn restore_port(&mut self, port: PortIndex, saved: PortSnapshot) -> bool {
        match &mut self.ports[port.get()].device {
            Some(d) => {
                d.published = saved.published;
                d.polling = saved.polling;
                true
            }
            None => false,
        }
    }

    /// A handle on the device instance now on `port`, for a handle a
    /// machine snapshot loads. On an empty port it is stale from the start
    /// and stays stale after an insertion, like a handle to a pad that was
    /// unplugged.
    pub fn reopen(&self, port: PortIndex) -> Handle {
        Handle {
            port,
            generation: self.ports[port.get()].generation,
        }
    }
}

/// A port's device as a machine snapshot keeps it
/// ([`InputManager::port_snapshot`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortSnapshot {
    pub published: State,
    pub polling: PollingParameters,
}

fn publish(d: &mut Device) {
    if d.latest != d.published.gamepad {
        d.published.gamepad = d.latest;
        d.published.packet_number = d.published.packet_number.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::Buttons;
    use crate::host::{HostButton, HostPad, HostPadState};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Shared {
        pads: Vec<HostPad>,
        rumble: Vec<(HostPadId, Rumble)>,
    }
    struct Mock(Arc<Mutex<Shared>>);
    impl HostBackend for Mock {
        fn name(&self) -> &str {
            "mock"
        }
        fn poll(&mut self) -> Vec<HostPad> {
            self.0.lock().unwrap().pads.clone()
        }
        fn set_rumble(&mut self, id: HostPadId, r: Rumble) {
            self.0.lock().unwrap().rumble.push((id, r));
        }
    }

    fn pad(id: u64, south: bool) -> HostPad {
        let mut state = HostPadState::default();
        state.buttons.set(HostButton::South, south);
        HostPad {
            id: HostPadId(id),
            name: "p".into(),
            state,
            physical: false,
            supports_rumble: true,
        }
    }

    fn setup() -> (InputManager, Arc<Mutex<Shared>>) {
        let sh = Arc::new(Mutex::new(Shared::default()));
        let mut m = InputManager::new(ConvertConfig::default()).unwrap();
        m.add_backend(Box::new(Mock(sh.clone())));
        (m, sh)
    }

    fn p(i: usize) -> PortIndex {
        PortIndex::new(i).unwrap()
    }

    /// Clears A on every pad and remembers what it saw.
    struct Clear(Arc<Mutex<Vec<(usize, HostPadId)>>>);
    impl PadGate for Clear {
        fn filter(&mut self, pads: &mut [GatedPad]) {
            for g in pads {
                self.0.lock().unwrap().push(g.key);
                g.pad.state.buttons.set(HostButton::South, false);
            }
        }
    }

    #[test]
    fn a_gate_sees_each_pad_first_and_decides_what_the_ports_get() {
        let (mut m, sh) = setup();
        let seen = Arc::new(Mutex::new(Vec::new()));
        sh.lock().unwrap().pads = vec![pad(10, true), pad(11, true)];
        m.sync();
        let h = m.open(p(0)).unwrap();
        assert!(
            m.get_state(h)
                .unwrap()
                .gamepad
                .analog(crate::AnalogButton::A)
                > 0
        );
        m.set_gate(Box::new(Clear(Arc::clone(&seen))));
        m.sync();
        assert_eq!(
            m.get_state(h)
                .unwrap()
                .gamepad
                .analog(crate::AnalogButton::A),
            0
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec![(0, HostPadId(10)), (0, HostPadId(11))]
        );
        // Ports and connection are untouched by the gate.
        assert_eq!(m.connected_mask(), 0b11);
        assert_eq!(m.open(p(0)).unwrap(), h, "the same device instance");
    }

    /// Describes one controller of its own, as GameController does.
    struct Described;
    impl HostBackend for Described {
        fn name(&self) -> &str {
            "described"
        }
        fn poll(&mut self) -> Vec<HostPad> {
            Vec::new()
        }
        fn devices(&self) -> Vec<crate::report::DeviceInfo> {
            vec![crate::report::DeviceInfo {
                backend: "described".into(),
                id: 3,
                name: "DUALSHOCK 4 Wireless Controller".into(),
                profile: Some("dualshock4".into()),
                ..Default::default()
            }]
        }
    }

    #[test]
    fn the_host_report_has_each_pad_as_the_gate_got_it_and_each_device() {
        let (mut m, sh) = setup();
        m.add_backend(Box::new(Described));
        let report = m.host_report();
        assert_eq!(report.get().syncs, 0);
        sh.lock().unwrap().pads = vec![pad(10, true), pad(11, false)];
        m.set_gate(Box::new(Clear(Arc::default())));
        m.sync();
        let r = report.get();
        assert_eq!(r.syncs, 1);
        assert_eq!(r.pads.len(), 2);
        assert_eq!(r.pads[0].backend, "mock");
        assert_eq!(r.pads[0].key, (0, 10));
        assert_eq!(r.pads[0].port, Some(0));
        assert_eq!(r.pads[1].port, Some(1));
        assert!(
            r.pads[0].state.buttons.contains(HostButton::South),
            "before the gate cleared it"
        );
        assert_eq!(r.devices.len(), 1);
        assert_eq!(r.devices[0].profile.as_deref(), Some("dualshock4"));
        // Unplugged: gone at the next sync.
        sh.lock().unwrap().pads.clear();
        m.sync();
        let r = report.get();
        assert_eq!((r.syncs, r.pads.len()), (2, 0));
    }

    #[test]
    fn insertion_removal_and_changes() {
        let (mut m, sh) = setup();
        m.sync();
        assert_eq!(m.connected_mask(), 0);
        assert_eq!(m.open(p(0)), Err(InputError::NotConnected));
        sh.lock().unwrap().pads = vec![pad(10, false), pad(11, false)];
        m.sync();
        assert_eq!(m.connected_mask(), 0b11);
        assert_eq!(m.device_changes(), (0b11, 0));
        assert_eq!(m.device_changes(), (0, 0));
        // Remove first; second keeps port 1; a newcomer takes port 0.
        sh.lock().unwrap().pads = vec![pad(11, false)];
        m.sync();
        assert_eq!(m.device_changes(), (0, 0b01));
        sh.lock().unwrap().pads = vec![pad(11, false), pad(12, false)];
        m.sync();
        assert_eq!(m.connected_mask(), 0b11);
        assert_eq!(m.device_changes(), (0b01, 0));
    }

    #[test]
    fn unplug_replug_between_changes_reports_both_and_stales_handles() {
        let (mut m, sh) = setup();
        sh.lock().unwrap().pads = vec![pad(1, false)];
        m.sync();
        m.device_changes();
        let h = m.open(p(0)).unwrap();
        sh.lock().unwrap().pads = vec![];
        m.sync();
        sh.lock().unwrap().pads = vec![pad(1, false)];
        m.sync();
        assert_eq!(m.device_changes(), (1, 1));
        assert_eq!(m.get_state(h), Err(InputError::NotConnected));
        assert_eq!(InputError::NotConnected.win32(), 1167);
        assert!(m.get_state(m.open(p(0)).unwrap()).is_ok());
    }

    #[test]
    fn at_most_four_ports() {
        let (mut m, sh) = setup();
        sh.lock().unwrap().pads = (0..6).map(|i| pad(i, false)).collect();
        m.sync();
        assert_eq!(m.connected_mask(), 0b1111);
    }

    #[test]
    fn packet_number_only_increments_on_change() {
        let (mut m, sh) = setup();
        sh.lock().unwrap().pads = vec![pad(1, false)];
        m.sync();
        let h = m.open(p(0)).unwrap();
        assert_eq!(m.get_state(h).unwrap().packet_number, 0);
        m.sync();
        m.sync();
        assert_eq!(m.get_state(h).unwrap().packet_number, 0);
        sh.lock().unwrap().pads = vec![pad(1, true)];
        m.sync();
        m.sync();
        let s = m.get_state(h).unwrap();
        assert_eq!(s.packet_number, 1);
        assert_eq!(s.gamepad.analog[0], 255);
        sh.lock().unwrap().pads = vec![pad(1, false)];
        m.sync();
        assert_eq!(m.get_state(h).unwrap().packet_number, 2);
        assert_eq!(m.get_state(h).unwrap().gamepad.buttons, Buttons(0));
    }

    #[test]
    fn packet_number_wraps_on_the_next_published_change() {
        let (mut m, sh) = setup();
        sh.lock().unwrap().pads = vec![pad(1, false)];
        m.sync();
        let h = m.open(p(0)).unwrap();
        m.device_mut(h).unwrap().published.packet_number = u32::MAX;
        sh.lock().unwrap().pads = vec![pad(1, true)];
        m.sync();
        assert_eq!(m.get_state(h).unwrap().packet_number, 0);
        m.sync();
        assert_eq!(m.get_state(h).unwrap().packet_number, 0);
    }

    #[test]
    fn manual_poll_holds_snapshot_until_polled() {
        let (mut m, sh) = setup();
        sh.lock().unwrap().pads = vec![pad(1, false)];
        m.sync();
        let h = m.open(p(0)).unwrap();
        let mut pp = m.polling_parameters(h).unwrap();
        assert!(pp.auto_poll);
        pp.auto_poll = false;
        m.set_polling_parameters(h, pp).unwrap();
        sh.lock().unwrap().pads = vec![pad(1, true)];
        m.sync();
        assert_eq!(m.get_state(h).unwrap().packet_number, 0);
        m.poll(h).unwrap();
        assert_eq!(m.get_state(h).unwrap().packet_number, 1);
        m.poll(h).unwrap();
        assert_eq!(m.get_state(h).unwrap().packet_number, 1);
    }

    fn req(addr: u32, l: u16, r: u16) -> FeedbackRequest {
        FeedbackRequest {
            addr,
            h_event: addr + 1,
            rumble: Rumble {
                left_motor: l,
                right_motor: r,
            },
        }
    }

    #[test]
    fn each_feedback_request_completes_independently_and_last_rumble_wins() {
        let (mut m, sh) = setup();
        sh.lock().unwrap().pads = vec![pad(7, false)];
        m.sync();
        let h = m.open(p(0)).unwrap();
        assert_eq!(
            m.set_state(h, req(0x1000, 1000, 0)).unwrap(),
            ERROR_IO_PENDING
        );
        assert_eq!(m.set_state(h, req(0x2000, 5, 6)).unwrap(), ERROR_IO_PENDING);
        assert!(m.take_completions().is_empty());
        m.sync();
        m.sync();
        let c = m.take_completions();
        assert_eq!(
            c,
            vec![
                Completion {
                    addr: 0x1000,
                    h_event: 0x1001,
                    status: ERROR_SUCCESS
                },
                Completion {
                    addr: 0x2000,
                    h_event: 0x2001,
                    status: ERROR_SUCCESS
                },
            ]
        );
        assert!(m.take_completions().is_empty());
        assert_eq!(
            sh.lock().unwrap().rumble,
            vec![(
                HostPadId(7),
                Rumble {
                    left_motor: 5,
                    right_motor: 6
                }
            )]
        );
    }

    #[test]
    fn unplug_completes_pending_feedback_with_not_connected() {
        let (mut m, sh) = setup();
        sh.lock().unwrap().pads = vec![pad(7, false)];
        m.sync();
        let h = m.open(p(0)).unwrap();
        m.set_state(h, req(0x3000, 1, 1)).unwrap();
        sh.lock().unwrap().pads = vec![];
        m.sync();
        assert_eq!(
            m.take_completions(),
            vec![Completion {
                addr: 0x3000,
                h_event: 0x3001,
                status: ERROR_DEVICE_NOT_CONNECTED
            }]
        );
        assert!(sh.lock().unwrap().rumble.is_empty());
        assert_eq!(
            m.set_state(h, req(0x4000, 1, 1)),
            Err(InputError::NotConnected)
        );
    }

    #[test]
    fn device_changes_accumulate_between_reads() {
        let (mut m, sh) = setup();
        m.device_changes();
        sh.lock().unwrap().pads = vec![pad(1, false)];
        m.sync();
        sh.lock().unwrap().pads = vec![];
        m.sync();
        // Inserted and removed before the guest looked: both are reported.
        assert_eq!(m.device_changes(), (1, 1));
        assert_eq!(m.device_changes(), (0, 0));
    }

    #[test]
    fn host_id_change_between_syncs_is_replug() {
        // What the gilrs backend does when a pad disconnects and reconnects
        // with the same gamepad index between two syncs.
        let (mut m, sh) = setup();
        sh.lock().unwrap().pads = vec![pad(1, false)];
        m.sync();
        m.device_changes();
        let h = m.open(p(0)).unwrap();
        sh.lock().unwrap().pads = vec![pad(1 | 1 << 32, false)];
        m.sync();
        assert_eq!(m.device_changes(), (1, 1));
        assert_eq!(m.get_state(h), Err(InputError::NotConnected));
    }

    #[test]
    fn invalid_convert_config_is_rejected() {
        let bad = ConvertConfig {
            stick_deadzone: f32::NAN,
            trigger_deadzone: 0.0,
        };
        assert!(InputManager::new(bad).is_err());
    }

    #[test]
    fn keyboard_handle_feeds_the_manager() {
        use crate::backend::keyboard::KeyboardMouse;
        use crate::bindings::Bindings;
        let kb = KeyboardMouse::new(Bindings::default());
        let feed = kb.clone(); // the platform layer keeps this handle
        let mut m = InputManager::new(ConvertConfig::default()).unwrap();
        m.add_backend(Box::new(kb));
        m.sync();
        let h = m.open(p(0)).unwrap();
        assert_eq!(m.get_state(h).unwrap().packet_number, 0);
        feed.key_down("Space");
        feed.mouse_motion(10.0, 0.0);
        m.sync();
        let s = m.get_state(h).unwrap();
        assert_eq!(s.packet_number, 1);
        assert_eq!(s.gamepad.analog[0], 255);
        assert!(s.gamepad.thumb_rx > 0);
        feed.key_up("Space");
        m.sync();
        assert_eq!(m.get_state(h).unwrap().gamepad.analog[0], 0);
    }

    #[test]
    fn default_keyboard_pad_is_port_zero_and_completes_rumble() {
        let (mut m, feed) = InputManager::with_keyboard_pad(Bindings::default()).unwrap();
        assert_eq!(m.connected_mask(), 0, "nothing before the first sync");
        m.sync();
        assert_eq!(m.connected_mask(), 1);
        assert_eq!(m.device_changes(), (1, 0));
        let h = m.open(p(0)).unwrap();
        let caps = m.capabilities(h).unwrap();
        assert_eq!(
            caps.output,
            Rumble {
                left_motor: u16::MAX,
                right_motor: u16::MAX
            }
        );
        feed.key_down("Escape");
        m.sync();
        assert!(
            m.get_state(h)
                .unwrap()
                .gamepad
                .buttons
                .contains(Buttons::START)
        );
        assert_eq!(
            m.set_state(h, req(0x5000, 0x8000, 0x10)).unwrap(),
            ERROR_IO_PENDING
        );
        m.sync();
        assert_eq!(
            m.take_completions(),
            vec![Completion {
                addr: 0x5000,
                h_event: 0x5001,
                status: ERROR_SUCCESS
            }]
        );
        assert_eq!(
            feed.rumble(),
            Rumble {
                left_motor: 0x8000,
                right_motor: 0x10
            }
        );
        // A plain KeyboardMouse keeps the old policy: no motors, 50.
        let kb = KeyboardMouse::new(Bindings::default());
        let mut plain = InputManager::new(ConvertConfig::default()).unwrap();
        plain.add_backend(Box::new(kb));
        plain.sync();
        let h = plain.open(p(0)).unwrap();
        assert_eq!(plain.capabilities(h).unwrap().output, Rumble::default());
        assert_eq!(
            plain.set_state(h, req(0x6000, 1, 1)),
            Err(InputError::NotSupported)
        );
    }

    #[test]
    fn state_bytes_in_guest_layout() {
        let (mut m, sh) = setup();
        sh.lock().unwrap().pads = vec![pad(1, true)];
        m.sync();
        let s = m.get_state(m.open(p(0)).unwrap()).unwrap();
        let b = s.to_bytes();
        assert_eq!(&b[0..4], &1u32.to_le_bytes());
        assert_eq!(b[4 + 2], 255);
    }

    #[test]
    fn a_restored_port_continues_the_saved_packet_numbers() {
        let (mut a, sh_a) = setup();
        sh_a.lock().unwrap().pads = vec![pad(1, true)];
        a.sync();
        sh_a.lock().unwrap().pads = vec![pad(1, false)];
        a.sync();
        let h = a.open(p(0)).unwrap();
        let polling = PollingParameters {
            auto_poll: false,
            interrupt_out: true,
            input_interval: 4,
            output_interval: 16,
        };
        a.set_polling_parameters(h, polling).unwrap();
        let saved = a.port_snapshot(p(0)).unwrap();
        assert_eq!(saved.published.packet_number, 2);
        assert_eq!(saved.polling, polling);
        assert_eq!(a.port_snapshot(p(1)), None);

        // Another manager whose pad has seen no input loads it.
        let (mut b, sh_b) = setup();
        sh_b.lock().unwrap().pads = vec![pad(7, true)];
        b.sync();
        assert!(b.restore_port(p(0), saved));
        assert!(!b.restore_port(p(1), saved));
        let h = b.reopen(p(0));
        assert_eq!(b.peek_state(h).unwrap(), saved.published);
        assert_eq!(b.polling_parameters(h).unwrap(), polling);
        // Auto-poll is off, as saved: the host's pressed A shows only after
        // a poll, as the saved run's next packet.
        b.sync();
        assert_eq!(b.peek_state(h).unwrap(), saved.published);
        b.poll(h).unwrap();
        assert_eq!(b.peek_state(h).unwrap().packet_number, 3);

        // A handle reopened on an empty port stays unplugged after a pad
        // arrives there.
        let stale = b.reopen(p(1));
        assert_eq!(b.peek_state(stale), Err(InputError::NotConnected));
        sh_b.lock().unwrap().pads = vec![pad(7, true), pad(8, false)];
        b.sync();
        assert!(b.open(p(1)).is_ok());
        assert_eq!(b.peek_state(stale), Err(InputError::NotConnected));
    }
}
