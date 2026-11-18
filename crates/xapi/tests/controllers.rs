mod common;

use common::*;
use input::gamepad::{Feedback, Rumble, State};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, mpsc},
    thread,
    time::Duration,
};
use xapi::{Error, Events, GuestMemory, Result, ThreadState, adapter::Function, memory};

#[test]
fn initialization_changes_and_descriptor_are_coherent() {
    let (mut xapi, mut memory, _) = setup();
    let mut thread = ThreadState::default();
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XInitDevices,
            &[0, 0]
        )
        .unwrap()
        .0,
        0xdead_beef
    );
    assert_eq!(
        (memory.word(0x100), memory.word(0x104), memory.word(0x108)),
        (1, 1, 0)
    );
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XGetDevices,
            &[0x100]
        )
        .unwrap()
        .0,
        1
    );
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XGetDevices,
            &[1]
        )
        .unwrap()
        .0,
        0
    );
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XGetDeviceChanges,
            &[0x100, 0x400, 0x404]
        )
        .unwrap()
        .0,
        1
    );
    assert_eq!((memory.word(0x400), memory.word(0x404)), (1, 0));
    assert_eq!((memory.word(0x104), memory.word(0x108)), (0, 1));
    assert!(
        !xapi
            .get_device_changes(&mut memory, 0x100, 0x400, 0x404)
            .unwrap()
    );
    assert_eq!((memory.word(0x400), memory.word(0x404)), (0, 0));
}

#[test]
fn plug_then_unplug_between_polls_is_not_reported_as_insertion() {
    let (mut xapi, mut memory, pads) = setup();
    pads.present(false);
    xapi.init_devices(&mut memory, 0, 0).unwrap();
    pads.present(true);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    pads.present(false);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    assert_eq!(
        (memory.word(0x100), memory.word(0x104), memory.word(0x108)),
        (0, 1, 0)
    );
    assert!(
        !xapi
            .get_device_changes(&mut memory, 0x100, 0x400, 0x404)
            .unwrap()
    );
    assert_eq!((memory.word(0x400), memory.word(0x404)), (0, 0));
    assert_eq!((memory.word(0x104), memory.word(0x108)), (0, 0));
}

#[test]
fn insert_remove_insert_with_no_previous_connection_reports_only_insertion() {
    let (mut xapi, mut memory, pads) = initialized();
    pads.present(false);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    pads.present(true);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    assert!(
        xapi.get_device_changes(&mut memory, 0x100, 0x400, 0x404)
            .unwrap()
    );
    assert_eq!((memory.word(0x400), memory.word(0x404)), (1, 0));
}

#[test]
fn removal_and_reinsert_of_previously_seen_pad_reports_both_and_peek_does_not_consume() {
    let (mut xapi, mut memory, pads) = initialized();
    xapi.get_device_changes(&mut memory, 0x100, 0x400, 0x404)
        .unwrap();
    pads.present(false);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    pads.present(true);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    let mut thread = ThreadState::default();
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XPeekDevices,
            &[0x100, 0x400, 0x404]
        )
        .unwrap()
        .0,
        1
    );
    assert_eq!((memory.word(0x400), memory.word(0x404)), (1, 1));
    assert!(
        xapi.get_device_changes(&mut memory, 0x100, 0x400, 0x404)
            .unwrap()
    );
    assert_eq!((memory.word(0x400), memory.word(0x404)), (1, 1));
    assert_eq!(xapi.peek_devices(&mut memory, 0x100, 0, 0).unwrap(), 1);
}

#[test]
fn invalid_outputs_do_not_consume_changes() {
    let (mut xapi, mut memory, _) = initialized();
    assert!(matches!(
        xapi.get_device_changes(&mut memory, 0x100, 0x400, 0x7fff),
        Err(Error::Memory(_))
    ));
    assert!(
        xapi.get_device_changes(&mut memory, 0x100, 0x400, 0x404)
            .unwrap()
    );
    assert_eq!(memory.word(0x400), 1);
    assert!(
        !xapi
            .get_device_changes(&mut memory, 0x999, 0x400, 0x404)
            .unwrap()
    );
}

#[test]
fn input_state_capabilities_poll_and_close_have_packed_outputs() {
    let (mut xapi, mut memory, pads) = initialized();
    let mut thread = ThreadState::default();
    let handle = call(
        &mut xapi,
        &mut memory,
        &mut thread,
        Function::XInputOpen,
        &[0x100, 0, 0, 0],
    )
    .unwrap()
    .0;
    pads.south(true);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XInputGetState,
            &[handle, 0x400]
        )
        .unwrap()
        .0,
        0
    );
    assert_eq!(memory.word(0x400), 1);
    assert_eq!((memory.bytes[0x406], memory.bytes[0x416]), (255, 0xcc));
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XInputGetCapabilities,
            &[handle, 0x500]
        )
        .unwrap()
        .0,
        0
    );
    assert_eq!((memory.bytes[0x500], memory.bytes[0x519]), (1, 0xcc));
    assert_eq!(&memory.bytes[0x515..0x519], &[255; 4]);
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XInputPoll,
            &[handle]
        )
        .unwrap()
        .0,
        0
    );
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XInputClose,
            &[handle]
        )
        .unwrap()
        .0,
        0xdead_beef
    );
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XInputGetState,
            &[handle, 0x400]
        )
        .unwrap()
        .0,
        6
    );
}

#[test]
fn disconnected_get_state_writes_final_report_before_1167_and_survives_replug() {
    let (mut xapi, mut memory, pads) = initialized();
    let old_handle = open(&mut xapi, &memory);
    pads.south(true);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    // Never call GetState while connected: the snapshot must be kept at sync.
    pads.present(false);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    let mut thread = ThreadState { last_error: 71 };
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XInputGetState,
            &[old_handle, 0x400]
        )
        .unwrap()
        .0,
        1167
    );
    let final_report = memory::read::<{ State::SIZE }>(&memory, 0x400).unwrap();
    assert_eq!(memory.word(0x400), 1);
    assert_eq!(final_report[6], 255);
    pads.present(true);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    let new_handle = open(&mut xapi, &memory);
    xapi.input_get_state(&mut memory, new_handle, 0x500)
        .unwrap();
    assert_eq!(memory.word(0x500), 0);
    assert_eq!(
        xapi.input_get_state(&mut memory, old_handle, 0x600),
        Err(Error::Win32(1167))
    );
    assert_eq!(
        memory::read::<{ State::SIZE }>(&memory, 0x600).unwrap(),
        final_report
    );
    assert_eq!(thread.last_error, 71);
}

#[test]
fn manual_polling_and_invalid_open_parameters() {
    let (mut xapi, mut memory, pads) = initialized();
    memory.put(0x600, &[0, 8, 8, 0]);
    let handle = xapi.input_open(&memory, 0x100, 0, 0, 0x600).unwrap();
    pads.south(true);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    xapi.input_get_state(&mut memory, handle, 0x400).unwrap();
    assert_eq!(memory.word(0x400), 0);
    xapi.input_poll(handle).unwrap();
    xapi.input_get_state(&mut memory, handle, 0x400).unwrap();
    assert_eq!(memory.word(0x400), 1);
    memory.put(0x600, &[4, 8, 8, 0]);
    assert_eq!(
        xapi.input_open(&memory, 0x100, 0, 0, 0x600),
        Err(Error::INVALID_PARAMETER)
    );
    for (device_type, port, slot) in [(1, 0, 0), (0x100, 4, 0), (0x100, 0, 1)] {
        assert_eq!(
            xapi.input_open(&memory, device_type, port, slot, 0),
            Err(Error::INVALID_PARAMETER)
        );
    }
    assert_eq!(
        xapi.init_devices(&mut memory, 1, 0),
        Err(Error::INVALID_PARAMETER)
    );
}

fn feedback(memory: &mut Memory, address: u32, event: u32) {
    memory.put(
        address,
        &Feedback {
            status: 42,
            h_event: event,
            rumble: Rumble {
                left_motor: 20,
                right_motor: 30,
            },
        }
        .to_bytes(),
    );
}

#[test]
fn rumble_statuses_are_completed_and_events_are_signalled_once() {
    let (mut xapi, mut memory, _) = initialized();
    let handle = open(&mut xapi, &memory);
    let mut thread = ThreadState { last_error: 88 };
    for (address, event) in [(0x600, 7), (0x700, 8)] {
        feedback(&mut memory, address, event);
        assert_eq!(
            call(
                &mut xapi,
                &mut memory,
                &mut thread,
                Function::XInputSetState,
                &[handle, address]
            )
            .unwrap()
            .0,
            997
        );
    }
    assert_eq!(
        xapi.input_set_state(&mut memory, handle, 0x600),
        Err(Error::Win32(170))
    );
    let mut signals = Signals::default();
    sync(&mut xapi, &mut memory, &mut signals).unwrap();
    sync(&mut xapi, &mut memory, &mut signals).unwrap();
    assert_eq!(signals.calls, [7, 8]);
    assert_eq!((memory.word(0x600), memory.word(0x700)), (0, 0));
    assert_eq!(thread.last_error, 88);
}

#[test]
fn pad_without_rumble_returns_50_and_advertises_no_motors() {
    let (mut xapi, mut memory, pads) = setup();
    pads.rumble(false);
    xapi.init_devices(&mut memory, 0, 0).unwrap();
    let handle = open(&mut xapi, &memory);
    xapi.input_get_capabilities(&mut memory, handle, 0x500)
        .unwrap();
    assert_eq!(&memory.bytes[0x515..0x519], &[0; 4]);
    feedback(&mut memory, 0x600, 7);
    let mut thread = ThreadState { last_error: 88 };
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XInputSetState,
            &[handle, 0x600]
        )
        .unwrap()
        .0,
        50
    );
    let mut signals = Signals::default();
    assert_eq!(memory.word(0x600), 50, "retail 0x0040a923");
    sync(&mut xapi, &mut memory, &mut signals).unwrap();
    assert_eq!(memory.word(0x600), 50);
    assert!(signals.calls.is_empty());
    assert_eq!(thread.last_error, 88);
}

#[test]
fn permanent_event_failure_cannot_block_later_or_new_completions() {
    let (mut xapi, mut memory, _) = initialized();
    let handle = open(&mut xapi, &memory);
    for (address, event) in [(0x600, 7), (0x700, 8)] {
        feedback(&mut memory, address, event);
        xapi.input_set_state(&mut memory, handle, address).unwrap();
    }
    let mut signals = Signals {
        failed_handle: Some(7),
        ..Signals::default()
    };
    assert_eq!(
        sync(&mut xapi, &mut memory, &mut signals),
        Err(Error::INVALID_HANDLE)
    );
    assert_eq!(signals.calls, [8]);
    feedback(&mut memory, 0x800, 9);
    xapi.input_set_state(&mut memory, handle, 0x800).unwrap();
    assert_eq!(
        sync(&mut xapi, &mut memory, &mut signals),
        Err(Error::INVALID_HANDLE)
    );
    assert_eq!(signals.calls, [8, 9]);
    signals.failed_handle = None;
    sync(&mut xapi, &mut memory, &mut signals).unwrap();
    assert_eq!(signals.calls, [8, 9, 7]);
}

#[test]
fn faulting_completion_buffer_does_not_block_other_buffers() {
    let (mut xapi, mut memory, _) = initialized();
    let handle = open(&mut xapi, &memory);
    for (address, event) in [(0x600, 7), (0x700, 8)] {
        feedback(&mut memory, address, event);
        xapi.input_set_state(&mut memory, handle, address).unwrap();
    }
    memory.readonly = Some((0x600, 4));
    let mut signals = Signals::default();
    assert!(matches!(
        sync(&mut xapi, &mut memory, &mut signals),
        Err(Error::Memory(_))
    ));
    assert_eq!(signals.calls, [8]);
    memory.readonly = None;
    sync(&mut xapi, &mut memory, &mut signals).unwrap();
    assert_eq!(signals.calls, [8, 7]);
}

#[test]
fn unplug_completes_feedback_with_disconnected_status() {
    let (mut xapi, mut memory, pads) = initialized();
    let handle = open(&mut xapi, &memory);
    feedback(&mut memory, 0x600, 9);
    xapi.input_set_state(&mut memory, handle, 0x600).unwrap();
    pads.present(false);
    let mut signals = Signals::default();
    sync(&mut xapi, &mut memory, &mut signals).unwrap();
    assert_eq!(memory.word(0x600), 1167);
    assert_eq!(signals.calls, [9]);
}

#[test]
fn output_faults_precede_feedback_side_effects() {
    let (mut xapi, mut memory, _) = initialized();
    let handle = open(&mut xapi, &memory);
    feedback(&mut memory, 0x600, 0);
    memory.readonly = Some((0x600, 70));
    assert!(matches!(
        xapi.input_set_state(&mut memory, handle, 0x600),
        Err(Error::Memory(_))
    ));
    memory.readonly = None;
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    assert_eq!(memory.word(0x600), 42);
    assert!(matches!(
        xapi.input_get_capabilities(&mut memory, handle, 0x7fff),
        Err(Error::Memory(_))
    ));
    assert!(matches!(
        xapi.input_get_state(&mut memory, handle, 0x7fff),
        Err(Error::Memory(_))
    ));
}

#[test]
fn unsupported_feedback_publishes_error_status() {
    let (mut xapi, mut memory, pads) = setup();
    pads.rumble(false);
    xapi.init_devices(&mut memory, 0, 0).unwrap();
    let handle = open(&mut xapi, &memory);
    feedback(&mut memory, 0x600, 7);
    memory.put(0x600, &997u32.to_le_bytes());
    assert_eq!(
        xapi.input_set_state(&mut memory, handle, 0x600),
        Err(Error::Win32(50))
    );
    assert_eq!(memory.word(0x600), 50, "retail 0x0040a923 writes 50");
}

#[test]
fn disconnected_feedback_publishes_error_status() {
    let (mut xapi, mut memory, pads) = initialized();
    let handle = open(&mut xapi, &memory);
    pads.present(false);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    feedback(&mut memory, 0x600, 7);
    memory.put(0x600, &997u32.to_le_bytes());
    assert_eq!(
        xapi.input_set_state(&mut memory, handle, 0x600),
        Err(Error::Win32(1167))
    );
    assert_eq!(memory.word(0x600), 1167, "retail 0x0040aa0a writes 1167");
    let mut signals = Signals::default();
    sync(&mut xapi, &mut memory, &mut signals).unwrap();
    assert!(signals.calls.is_empty());
}

/// Bridges XAPI over one guard of the runtime's non-reentrant memory mutex.
struct Guarded<'a>(MutexGuard<'a, Memory>);

impl GuestMemory for Guarded<'_> {
    fn check(&self, address: u32, length: usize, write: bool) -> Result<()> {
        self.0.check(address, length, write)
    }

    fn read(&self, address: u32, bytes: &mut [u8]) -> Result<()> {
        self.0.read(address, bytes)
    }

    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<()> {
        self.0.write(address, bytes)
    }
}

/// Models kernel DispatcherReference::signal_event: the guest handle was
/// resolved to a pinned event object at submission, and signalling relocks
/// guest memory to set the dispatcher header's SignalState.
struct KernelEvents {
    memory: Arc<Mutex<Memory>>,
    pinned: HashMap<u32, u32>,
}

impl Events for KernelEvents {
    fn signal(&mut self, guest_handle: u32) -> Result<()> {
        let object = *self
            .pinned
            .get(&guest_handle)
            .ok_or(Error::INVALID_HANDLE)?;
        let mut memory = self.memory.lock().unwrap();
        memory.write(object + 4, &1u32.to_le_bytes())?;
        self.pinned.remove(&guest_handle);
        Ok(())
    }
}

#[test]
fn pinned_kernel_event_is_signalled_after_memory_guard_is_released() {
    let (finished, outcome) = mpsc::channel();
    thread::spawn(move || {
        let (mut xapi, memory, _) = setup();
        let memory = Arc::new(Mutex::new(memory));
        let mut events = KernelEvents {
            memory: Arc::clone(&memory),
            pinned: HashMap::from([(0x44, 0x900)]),
        };
        let handle = {
            let mut guest = Guarded(memory.lock().unwrap());
            guest.0.put(0x900, &[0; 16]);
            xapi.init_devices(&mut guest, 0, 0).unwrap();
            let handle = xapi.input_open(&guest, 0x100, 0, 0, 0).unwrap();
            feedback(&mut guest.0, 0x600, 0x44);
            assert_eq!(xapi.input_set_state(&mut guest, handle, 0x600), Ok(997));
            handle
        };
        let written = xapi.sync_input(&mut Guarded(memory.lock().unwrap()));
        let report = xapi.take_signals().deliver(&mut events);
        let delivered = written.and(xapi.acknowledge_signals(report));
        let mut guest = Guarded(memory.lock().unwrap());
        // A finished buffer is the guest's again and may be resubmitted.
        let resubmitted = xapi.input_set_state(&mut guest, handle, 0x600);
        let _ = finished.send((
            delivered,
            guest.0.word(0x900 + 4),
            events.pinned.is_empty(),
            resubmitted,
        ));
    });
    let (delivered, signal_state, released, resubmitted) = outcome
        .recv_timeout(Duration::from_secs(30))
        .expect("completion deadlocked signalling a kernel event under the memory guard");
    assert_eq!(delivered, Ok(()));
    assert_eq!(signal_state, 1);
    assert!(released, "pinned event must be released after signalling");
    assert_eq!(resubmitted, Ok(997));
}

#[test]
fn feedback_stays_pending_until_signal_is_acknowledged() {
    let (mut xapi, mut memory, _) = initialized();
    let handle = open(&mut xapi, &memory);
    feedback(&mut memory, 0x600, 7);
    xapi.input_set_state(&mut memory, handle, 0x600).unwrap();
    xapi.sync_input(&mut memory).unwrap();
    assert_eq!(memory.word(0x600), 0, "status precedes the signal");
    let batch = xapi.take_signals();
    assert_eq!(batch.signals().len(), 1);
    assert_eq!(
        xapi.input_set_state(&mut memory, handle, 0x600),
        Err(Error::Win32(170))
    );
    let mut signals = Signals::default();
    xapi.acknowledge_signals(batch.deliver(&mut signals))
        .unwrap();
    assert_eq!(signals.calls, [7]);
    assert!(xapi.take_signals().is_empty());
    assert_eq!(xapi.input_set_state(&mut memory, handle, 0x600), Ok(997));
}

#[test]
fn an_input_tap_records_what_the_guest_read_and_a_replay_gives_it_back() {
    use xapi::tap::{Recorder, Replayer};
    let (mut xapi, mut memory, pads) = initialized();
    memory.put(0x600, &[0, 8, 8, 0]);
    let handle = xapi.input_open(&memory, 0x100, 0, 0, 0x600).unwrap();
    let (recorder, log) = Recorder::new();
    xapi.set_input_tap(Some(Box::new(recorder)));
    let mut seen = Vec::new();
    for pressed in [false, true, true, false] {
        pads.south(pressed);
        sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
        xapi.input_poll(handle).unwrap();
        xapi.input_get_state(&mut memory, handle, 0x400).unwrap();
        seen.push(memory::read::<{ State::SIZE }>(&memory, 0x400).unwrap());
    }
    let log = log.lock().unwrap().clone();
    assert_eq!(
        log.reads.iter().map(|r| r.packet).collect::<Vec<_>>(),
        seen,
        "the log holds exactly the bytes the guest got"
    );
    assert!(
        log.reads
            .iter()
            .all(|r| r.status == 0 && r.handle == handle)
    );
    assert_ne!(seen[0], seen[1]);

    // Replay with the live pad held down throughout: the guest still reads
    // the recording, read for read.
    let (replayer, stats) = Replayer::new(log.clone());
    xapi.set_input_tap(Some(Box::new(replayer)));
    pads.south(true);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    xapi.input_poll(handle).unwrap();
    for expected in &seen {
        xapi.input_get_state(&mut memory, handle, 0x400).unwrap();
        assert_eq!(
            &memory::read::<{ State::SIZE }>(&memory, 0x400).unwrap(),
            expected
        );
    }
    assert_eq!(stats.lock().unwrap().replayed, 4);

    // A recorded failure status comes back as the call's error.
    let mut failing = log;
    failing.reads[0].status = 1167;
    let (replayer, _) = Replayer::new(failing);
    xapi.set_input_tap(Some(Box::new(replayer)));
    assert_eq!(
        xapi.input_get_state(&mut memory, handle, 0x400),
        Err(Error::Win32(1167))
    );
    assert_eq!(
        memory::read::<{ State::SIZE }>(&memory, 0x400).unwrap(),
        seen[0]
    );
}

#[test]
fn an_input_tap_is_told_which_port_each_handle_opened_and_when_it_closed() {
    use xapi::tap::{InputTap, PACKET};
    #[derive(Default)]
    struct Seen(Arc<Mutex<Vec<String>>>);
    impl InputTap for Seen {
        fn read(&mut self, handle: u32, _: &mut u32, _: &mut [u8; PACKET]) {
            self.0.lock().unwrap().push(format!("read {handle:x}"));
        }
        fn opened(&mut self, handle: u32, port: u32) {
            self.0
                .lock()
                .unwrap()
                .push(format!("open {handle:x} {port}"));
        }
        fn closed(&mut self, handle: u32) {
            self.0.lock().unwrap().push(format!("close {handle:x}"));
        }
    }
    let (mut xapi, mut memory, _pads) = initialized();
    let seen = Seen::default();
    let log = Arc::clone(&seen.0);
    xapi.set_input_tap(Some(Box::new(seen)));
    let handle = xapi.input_open(&memory, 0x100, 0, 0, 0).unwrap();
    xapi.input_get_state(&mut memory, handle, 0x400).unwrap();
    xapi.input_close(handle).unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        [
            format!("open {handle:x} 0"),
            format!("read {handle:x}"),
            format!("close {handle:x}")
        ]
    );
    assert!(xapi.take_input_tap().is_some());
    assert!(xapi.take_input_tap().is_none());
}

#[test]
fn each_ports_last_state_is_readable_by_the_host() {
    // Voice chat's push-to-talk reads the pad the game last read, without
    // a guest call (docs/live/live-voice.md, 5 step 4).
    let (mut xapi, mut memory, pads) = initialized();
    assert_eq!(xapi.last_input_state(0), None, "not open");
    let handle = open(&mut xapi, &memory);
    pads.button(input::host::HostButton::Back, true);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    xapi.input_get_state(&mut memory, handle, 0x400).unwrap();
    let state = xapi.last_input_state(0).expect("port 0 is open");
    assert!(state.gamepad.buttons.contains(input::gamepad::Buttons::BACK));
    assert_eq!(state.to_bytes()[..], memory::read::<{ State::SIZE }>(&memory, 0x400).unwrap());
    assert_eq!(xapi.last_input_state(1), None);
    assert_eq!(xapi.last_input_state(9), None);
    pads.button(input::host::HostButton::Back, false);
    sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
    xapi.input_get_state(&mut memory, handle, 0x400).unwrap();
    assert!(!xapi.last_input_state(0).unwrap().gamepad.buttons.contains(input::gamepad::Buttons::BACK));
    xapi.input_close(handle).unwrap();
    assert_eq!(xapi.last_input_state(0), None, "closed");
}
