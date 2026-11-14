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

use std::collections::HashMap;

use crate::backend::HostBackend;
use crate::gamepad::{
    Capabilities, ERROR_DEVICE_NOT_CONNECTED, ERROR_IO_PENDING, ERROR_SUCCESS, Gamepad, PollingParameters,
    Rumble, State,
};
use crate::host::{ConfigError, ConvertConfig, HostPadId};

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
}

impl InputError {
    /// The Win32 code the guest should see.
    pub fn win32(&self) -> u32 {
        ERROR_DEVICE_NOT_CONNECTED
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
    pending: Vec<FeedbackRequest>,
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
        out.extend(self.pending.drain(..).map(|r| Completion { addr: r.addr, h_event: r.h_event, status }));
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
        })
    }

    pub fn add_backend(&mut self, b: Box<dyn HostBackend>) {
        self.backends.push(b);
    }

    /// Pull host state and update the ports: removals, insertions, auto-poll
    /// publishing and rumble forwarding.
    pub fn sync(&mut self) {
        let mut seen: HashMap<(usize, HostPadId), crate::host::HostPad> = HashMap::new();
        let mut order = Vec::new();
        for (bi, b) in self.backends.iter_mut().enumerate() {
            for pad in b.poll() {
                let key = (bi, pad.id);
                order.push(key);
                seen.insert(key, pad);
            }
        }

        // Removals.
        for (i, port) in self.ports.iter_mut().enumerate() {
            if port.device.as_ref().is_some_and(|d| !seen.contains_key(&d.host)) {
                let mut d = port.device.take().expect("checked");
                d.complete_all(ERROR_DEVICE_NOT_CONNECTED, &mut self.completions);
                self.changes_rem |= 1 << i;
            }
        }
        // Insertions, in backend order, lowest free port.
        for key in &order {
            if self.ports.iter().any(|p| p.device.as_ref().is_some_and(|d| d.host == *key)) {
                continue;
            }
            let Some(i) = self.ports.iter().position(|p| p.device.is_none()) else {
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
                pending: Vec::new(),
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
    }

    /// `XGetDevices(XDEVICE_TYPE_GAMEPAD)`: bitmask of connected ports.
    pub fn connected_mask(&self) -> u32 {
        (0..PORT_COUNT).filter(|&i| self.ports[i].device.is_some()).fold(0, |m, i| m | 1 << i)
    }

    /// `XGetDeviceChanges`: `(insertions, removals)` since the previous call,
    /// accumulated at each `sync`. A port unplugged and replugged (or plugged
    /// and unplugged) in between shows up in both.
    pub fn device_changes(&mut self) -> (u32, u32) {
        (std::mem::take(&mut self.changes_ins), std::mem::take(&mut self.changes_rem))
    }

    /// Drain finished feedback requests.
    pub fn take_completions(&mut self) -> Vec<Completion> {
        std::mem::take(&mut self.completions)
    }

    /// `XInputOpen`.
    pub fn open(&self, port: PortIndex) -> Result<Handle, InputError> {
        let p = &self.ports[port.get()];
        p.device.as_ref().map(|_| Handle { port, generation: p.generation }).ok_or(InputError::NotConnected)
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

    /// `XInputGetState`.
    pub fn get_state(&self, h: Handle) -> Result<State, InputError> {
        self.device(h).map(|d| d.published)
    }

    /// `XInputPoll`: publish the newest host state (matters when auto-poll is
    /// off; harmless otherwise).
    pub fn poll(&mut self, h: Handle) -> Result<(), InputError> {
        publish(self.device_mut(h)?);
        Ok(())
    }

    /// `XInputGetCapabilities`.
    pub fn capabilities(&self, h: Handle) -> Result<Capabilities, InputError> {
        self.device(h).map(|_| Capabilities::standard())
    }

    pub fn polling_parameters(&self, h: Handle) -> Result<PollingParameters, InputError> {
        self.device(h).map(|d| d.polling)
    }

    /// `XInputSetPollingParameters`.
    pub fn set_polling_parameters(&mut self, h: Handle, p: PollingParameters) -> Result<(), InputError> {
        self.device_mut(h)?.polling = p;
        Ok(())
    }

    /// `XInputSetState`: queue a rumble request. Returns `ERROR_IO_PENDING`
    /// (the value for the call and the initial `dwStatus`); the request later
    /// appears in [`take_completions`](Self::take_completions).
    pub fn set_state(&mut self, h: Handle, req: FeedbackRequest) -> Result<u32, InputError> {
        self.device_mut(h)?.pending.push(req);
        Ok(ERROR_IO_PENDING)
    }
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
        HostPad { id: HostPadId(id), name: "p".into(), state, physical: false, supports_rumble: true }
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
        FeedbackRequest { addr, h_event: addr + 1, rumble: Rumble { left_motor: l, right_motor: r } }
    }

    #[test]
    fn each_feedback_request_completes_independently_and_last_rumble_wins() {
        let (mut m, sh) = setup();
        sh.lock().unwrap().pads = vec![pad(7, false)];
        m.sync();
        let h = m.open(p(0)).unwrap();
        assert_eq!(m.set_state(h, req(0x1000, 1000, 0)).unwrap(), ERROR_IO_PENDING);
        assert_eq!(m.set_state(h, req(0x2000, 5, 6)).unwrap(), ERROR_IO_PENDING);
        assert!(m.take_completions().is_empty());
        m.sync();
        m.sync();
        let c = m.take_completions();
        assert_eq!(
            c,
            vec![
                Completion { addr: 0x1000, h_event: 0x1001, status: ERROR_SUCCESS },
                Completion { addr: 0x2000, h_event: 0x2001, status: ERROR_SUCCESS },
            ]
        );
        assert!(m.take_completions().is_empty());
        assert_eq!(sh.lock().unwrap().rumble, vec![(HostPadId(7), Rumble { left_motor: 5, right_motor: 6 })]);
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
            vec![Completion { addr: 0x3000, h_event: 0x3001, status: ERROR_DEVICE_NOT_CONNECTED }]
        );
        assert!(sh.lock().unwrap().rumble.is_empty());
        assert_eq!(m.set_state(h, req(0x4000, 1, 1)), Err(InputError::NotConnected));
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
        let bad = ConvertConfig { stick_deadzone: f32::NAN, trigger_deadzone: 0.0 };
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
    fn state_bytes_in_guest_layout() {
        let (mut m, sh) = setup();
        sh.lock().unwrap().pads = vec![pad(1, true)];
        m.sync();
        let s = m.get_state(m.open(p(0)).unwrap()).unwrap();
        let b = s.to_bytes();
        assert_eq!(&b[0..4], &1u32.to_le_bytes());
        assert_eq!(b[4 + 2], 255);
    }
}
