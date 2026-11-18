//! Machine snapshots (docs/runtime/snapshots.md): the `xapi` section.
//! A machine saves its open pad handles, change words and the ports'
//! packet numbers; another booted the same way loads them and the guest
//! reads what it would have read on the first.
mod common;

use common::*;
use input::gamepad::{Feedback, State};
use snapshot::{BuildKey, Header, Snapshot, SnapshotWriter};
use xapi::snapshot::SnapshotError;
use xapi::{Error, Xapi, memory};

fn key() -> BuildKey {
    BuildKey {
        stamp: "xapi test".into(),
        switches: String::new(),
    }
}

fn save(xapi: &Xapi<Utility>) -> Result<Vec<u8>, SnapshotError> {
    let mut w = SnapshotWriter::new(Header {
        build: key(),
        name: "pads".into(),
        tick: 0,
        presents: 0,
        notes: Vec::new(),
    });
    xapi.save_snapshot(&mut w)?;
    Ok(w.finish().unwrap())
}

fn load(xapi: &mut Xapi<Utility>, bytes: &[u8]) {
    let snapshot = Snapshot::open(bytes, &key()).unwrap();
    xapi.load_snapshot(&snapshot).unwrap();
}

fn read(
    xapi: &mut Xapi<Utility>,
    memory: &mut Memory,
    handle: u32,
) -> (Result<(), Error>, State) {
    let result = xapi.input_get_state(memory, handle, 0x400);
    let bytes = memory::read::<{ State::SIZE }>(&*memory, 0x400).unwrap();
    let gamepad = <[u8; input::Gamepad::SIZE]>::try_from(&bytes[4..]).unwrap();
    let state = State {
        packet_number: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        gamepad: input::Gamepad::from_bytes(&gamepad),
    };
    (result, state)
}

/// A machine with pads consumed, one handle open on port 0 that has read
/// two packets, and a second handle opened and closed.
fn played() -> (Xapi<Utility>, Memory, Pads, u32) {
    let (mut xapi, mut memory, pads) = initialized();
    xapi.get_device_changes(&mut memory, 0x100, 0x500, 0x504)
        .unwrap();
    let handle = open(&mut xapi, &memory);
    let other = open(&mut xapi, &memory);
    xapi.input_close(other).unwrap();
    for pressed in [true, false] {
        pads.south(pressed);
        sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
        read(&mut xapi, &mut memory, handle).0.unwrap();
    }
    (xapi, memory, pads, handle)
}

#[test]
fn a_loaded_machine_reads_the_packets_and_handles_the_saved_one_would() {
    let (mut a, mut memory_a, pads_a, handle) = played();
    let bytes = save(&a).unwrap();

    // The original carries on: A pressed, then a new handle.
    pads_a.south(true);
    sync(&mut a, &mut memory_a, &mut Signals::default()).unwrap();
    let expected = read(&mut a, &mut memory_a, handle);
    assert_eq!(expected.1.packet_number, 3);
    let expected_token = open(&mut a, &memory_a);

    // A machine that has seen no input loads the snapshot and does the same.
    let (mut b, mut memory_b, pads_b) = initialized();
    b.get_device_changes(&mut memory_b, 0x100, 0x500, 0x504)
        .unwrap();
    load(&mut b, &bytes);
    assert_eq!(b.last_input_state(0).unwrap().packet_number, 2);
    pads_b.south(true);
    sync(&mut b, &mut memory_b, &mut Signals::default()).unwrap();
    assert_eq!(read(&mut b, &mut memory_b, handle), expected);
    assert_eq!(open(&mut b, &memory_b), expected_token);
    // Same pads on both: no change for the guest to see.
    assert!(
        !b.get_device_changes(&mut memory_b, 0x100, 0x500, 0x504)
            .unwrap()
    );
    // The handle closed before the save stays closed.
    assert_eq!(
        b.input_get_state(&mut memory_b, handle + 4, 0x400),
        Err(Error::INVALID_HANDLE)
    );
}

#[test]
fn a_handle_whose_port_is_empty_at_load_reads_as_unplugged() {
    let (a, _, _, handle) = played();
    let bytes = save(&a).unwrap();
    let saved = a.last_input_state(0).unwrap();

    let (mut b, mut memory_b, pads_b) = setup();
    pads_b.present(false);
    b.init_devices(&mut memory_b, 0, 0).unwrap();
    load(&mut b, &bytes);
    // The final state read before the save, with ERROR_DEVICE_NOT_CONNECTED.
    assert_eq!(
        read(&mut b, &mut memory_b, handle),
        (Err(Error::Win32(1167)), saved)
    );
    // The guest last saw port 0 connected: a removal it has yet to consume.
    assert!(
        b.get_device_changes(&mut memory_b, 0x100, 0x500, 0x504)
            .unwrap()
    );
    assert_eq!((memory_b.word(0x500), memory_b.word(0x504)), (0, 1));
    // A pad plugged in later is a new instance; the old handle stays stale.
    pads_b.present(true);
    sync(&mut b, &mut memory_b, &mut Signals::default()).unwrap();
    assert_eq!(
        read(&mut b, &mut memory_b, handle).0,
        Err(Error::Win32(1167))
    );
}

#[test]
fn rumble_in_flight_refuses_the_save_until_it_completes() {
    let (mut a, mut memory, _, handle) = played();
    memory.put(0x600, &[0; Feedback::SIZE]);
    a.input_set_state(&mut memory, handle, 0x600).unwrap();
    match save(&a) {
        Err(SnapshotError::Busy(why)) => {
            assert_eq!(why, ["1 rumble requests in flight"])
        }
        other => panic!("saved with rumble pending: {:?}", other.map(|b| b.len())),
    }
    // No event to signal: the next frame finishes the request.
    sync(&mut a, &mut memory, &mut Signals::default()).unwrap();
    assert!(a.snapshot_blockers().is_empty());
    save(&a).unwrap();
}

#[test]
fn a_damaged_section_changes_nothing() {
    let (_, _, _, handle) = played();
    let mut w = SnapshotWriter::new(Header {
        build: key(),
        name: "bad".into(),
        tick: 0,
        presents: 0,
        notes: Vec::new(),
    });
    // A handle on port 9.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x5800_0010u32.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&handle.to_le_bytes());
    bytes.push(9);
    bytes.extend_from_slice(&[0; State::SIZE]);
    w.section(xapi::snapshot::SECTION, 1, snapshot::Codec::Stored, bytes)
        .unwrap();
    let file = w.finish().unwrap();

    let (mut b, mut memory_b, _) = initialized();
    let kept = open(&mut b, &memory_b);
    let snapshot = Snapshot::open(&file, &key()).unwrap();
    assert!(matches!(
        b.load_snapshot(&snapshot),
        Err(SnapshotError::Codec(_))
    ));
    b.input_get_state(&mut memory_b, kept, 0x400).unwrap();
}
