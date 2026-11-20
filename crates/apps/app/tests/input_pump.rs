//! Window keys through crates/platform/input and crates/platform/xapi, and the lock order of
//! one input pump (docs/runtime/xapi.md, integration step 5).

use std::sync::{Mutex, MutexGuard};

use app::keymap::key_name;
use app::pump_input;
use input::backend::keyboard::KeyboardMouse;
use input::bindings::Bindings;
use input::gamepad::{Gamepad, State};
use input::host::ConvertConfig;
use input::{HostBackend, HostPad, HostPadId, HostPadState, InputManager};
use winit::keyboard::KeyCode;
use xapi::{Config, Error, Events, GuestMemory, MemoryFault, Result, Storage, Xapi};

const DEVICE_TYPE: u32 = 0x100;
const STATE_OUT: u32 = 0x400;
const FEEDBACK: u32 = 0x600;

struct Flat(Vec<u8>);

impl Flat {
    fn check(&self, address: u32, length: usize, write: bool) -> Result<()> {
        if address == 0 || address as usize + length > self.0.len() {
            return Err(Error::Memory(MemoryFault {
                address,
                length,
                write,
            }));
        }
        Ok(())
    }
}

/// The adapter the runtime would build: owns the memory guard.
struct Guard<'a>(MutexGuard<'a, Flat>);

impl GuestMemory for Guard<'_> {
    fn check(&self, address: u32, length: usize, write: bool) -> Result<()> {
        self.0.check(address, length, write)
    }
    fn read(&self, address: u32, bytes: &mut [u8]) -> Result<()> {
        self.0.check(address, bytes.len(), false)?;
        bytes.copy_from_slice(&self.0.0[address as usize..][..bytes.len()]);
        Ok(())
    }
    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<()> {
        self.0.check(address, bytes.len(), true)?;
        self.0.0[address as usize..][..bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
}

struct NoStorage;

impl Storage for NoStorage {
    fn mount_utility(&mut self, _: u8, _: bool) -> Result<()> {
        Ok(())
    }
    fn format_utility(&mut self) -> Result<()> {
        Ok(())
    }
}

/// A pad with rumble, so feedback completions (and event signals) happen.
struct RumblePad;

impl HostBackend for RumblePad {
    fn name(&self) -> &str {
        "rumble"
    }
    fn poll(&mut self) -> Vec<HostPad> {
        vec![HostPad {
            id: HostPadId(7),
            name: "rumble".into(),
            state: HostPadState::default(),
            physical: false,
            supports_rumble: true,
        }]
    }
}

/// Records signals and proves neither guard is held while signalling.
struct Signals<'a> {
    memory: &'a Mutex<Flat>,
    xapi: &'a Mutex<Xapi<NoStorage>>,
    seen: Vec<u32>,
}

impl Events for Signals<'_> {
    fn signal(&mut self, handle: u32) -> Result<()> {
        assert!(
            self.memory.try_lock().is_ok(),
            "guest memory still locked during delivery"
        );
        assert!(
            self.xapi.try_lock().is_ok(),
            "XAPI still locked during delivery"
        );
        self.seen.push(handle);
        Ok(())
    }
}

fn setup(backends: Vec<Box<dyn HostBackend>>) -> (Mutex<Xapi<NoStorage>>, Mutex<Flat>) {
    let mut input = InputManager::new(ConvertConfig::default()).unwrap();
    for b in backends {
        input.add_backend(b);
    }
    let xapi = Xapi::new(Config::for_gamepad(DEVICE_TYPE), input, NoStorage).unwrap();
    (Mutex::new(xapi), Mutex::new(Flat(vec![0; 0x1000])))
}

fn read_state(xapi: &Mutex<Xapi<NoStorage>>, memory: &Mutex<Flat>, handle: u32) -> State {
    let mut g = Guard(memory.lock().unwrap());
    xapi.lock()
        .unwrap()
        .input_get_state(&mut g, handle, STATE_OUT)
        .unwrap();
    let mut bytes = [0u8; State::SIZE];
    g.read(STATE_OUT, &mut bytes).unwrap();
    State {
        packet_number: u32::from_le_bytes(bytes[..4].try_into().unwrap()),
        gamepad: Gamepad::from_bytes(bytes[4..].try_into().unwrap()),
    }
}

#[test]
fn window_keys_reach_the_guest_state() {
    let kb = KeyboardMouse::new(Bindings::default());
    let (xapi, memory) = setup(vec![Box::new(kb.clone())]);
    let mut events = Signals {
        memory: &memory,
        xapi: &xapi,
        seen: vec![],
    };

    // The pad appears on the first pump, then the guest opens port 0.
    pump_input(&xapi, Guard(memory.lock().unwrap()), &mut events).unwrap();
    let handle = {
        let g = Guard(memory.lock().unwrap());
        xapi.lock()
            .unwrap()
            .input_open(&g, DEVICE_TYPE, 0, 0, 0)
            .unwrap()
    };
    let idle = read_state(&xapi, &memory, handle);
    assert_eq!(idle.gamepad.analog[0], 0);

    // The window thread presses Space (A) and W (forward).
    kb.key_down(key_name(KeyCode::Space).unwrap());
    kb.key_down(key_name(KeyCode::KeyW).unwrap());
    let before = read_state(&xapi, &memory, handle);
    assert_eq!(
        before.gamepad.analog[0], 0,
        "nothing reaches the guest until a pump"
    );

    pump_input(&xapi, Guard(memory.lock().unwrap()), &mut events).unwrap();
    let pressed = read_state(&xapi, &memory, handle);
    assert_eq!(pressed.gamepad.analog[0], 255, "A pressed");
    assert_eq!(pressed.gamepad.thumb_ly, 32767, "full forward");
    assert!(pressed.packet_number > idle.packet_number);

    kb.release_all();
    pump_input(&xapi, Guard(memory.lock().unwrap()), &mut events).unwrap();
    let released = read_state(&xapi, &memory, handle);
    assert_eq!(released.gamepad.analog[0], 0);
    assert_eq!(released.gamepad.thumb_ly, 0);
}

#[test]
fn feedback_signals_are_delivered_without_guards_and_acknowledged() {
    let (xapi, memory) = setup(vec![Box::new(RumblePad)]);
    let mut events = Signals {
        memory: &memory,
        xapi: &xapi,
        seen: vec![],
    };
    pump_input(&xapi, Guard(memory.lock().unwrap()), &mut events).unwrap();
    let handle = {
        let mut g = Guard(memory.lock().unwrap());
        let h = xapi
            .lock()
            .unwrap()
            .input_open(&g, DEVICE_TYPE, 0, 0, 0)
            .unwrap();
        // XINPUT_FEEDBACK: dwStatus @0, hEvent @4, rumble @66.
        let mut fb = [0u8; 70];
        fb[4..8].copy_from_slice(&0x55u32.to_le_bytes());
        fb[66..70].copy_from_slice(&[0xff, 0xff, 0, 0]);
        g.write(FEEDBACK, &fb).unwrap();
        assert_eq!(
            xapi.lock().unwrap().input_set_state(&mut g, h, FEEDBACK),
            Ok(997)
        );
        h
    };
    let _ = handle;
    let report = pump_input(&xapi, Guard(memory.lock().unwrap()), &mut events).unwrap();
    assert_eq!(report.signals, 1);
    assert_eq!(events.seen, [0x55]);
    let status = {
        let g = Guard(memory.lock().unwrap());
        let mut b = [0u8; 4];
        g.read(FEEDBACK, &mut b).unwrap();
        u32::from_le_bytes(b)
    };
    assert_eq!(status, 0, "completed with ERROR_SUCCESS");

    // Acknowledged: the same buffer can be submitted again (170 if still pending).
    let mut g = Guard(memory.lock().unwrap());
    assert_eq!(
        xapi.lock()
            .unwrap()
            .input_set_state(&mut g, handle, FEEDBACK),
        Ok(997)
    );
    drop(g);
    let report = pump_input(&xapi, Guard(memory.lock().unwrap()), &mut events).unwrap();
    assert_eq!(report.signals, 1);
    assert_eq!(events.seen, [0x55, 0x55]);
}

#[test]
fn sync_errors_are_returned_after_signals_still_flow() {
    // The device-type object is out of bounds: sync_input fails, nothing panics,
    // and the error is reported to the caller.
    let kb = KeyboardMouse::new(Bindings::default());
    let mut input = InputManager::new(ConvertConfig::default()).unwrap();
    input.add_backend(Box::new(kb));
    let xapi = Mutex::new(Xapi::new(Config::for_gamepad(0xfff0), input, NoStorage).unwrap());
    let memory = Mutex::new(Flat(vec![0; 0x1000]));
    let mut events = Signals {
        memory: &memory,
        xapi: &xapi,
        seen: vec![],
    };
    let r = pump_input(&xapi, Guard(memory.lock().unwrap()), &mut events);
    assert!(matches!(r, Err(Error::Memory(_))), "{r:?}");
}
