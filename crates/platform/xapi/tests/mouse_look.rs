//! Mouse look through XAPI, the way `h2 play` drives it: `sync_input` after
//! every guest slice (host rate) and `XInputGetState` once per game frame
//! (about 5 fps today). `sync_input` also refreshes each handle's last state;
//! that bookkeeping must not count as the guest reading the pad.

mod common;

use common::*;
use input::InputManager;
use input::backend::fallback::KeyboardFallback;
use input::backend::keyboard::KeyboardMouse;
use input::bindings::Bindings;
use input::host::ConvertConfig;
use xapi::{Config, ThreadState, Xapi, adapter::Function};

const OUT: u32 = 0x400;

fn thumb_r(memory: &Memory) -> (i16, i16) {
    // XINPUT_STATE: dwPacketNumber, then XINPUT_GAMEPAD with sThumbRX/RY at
    // offsets 14 and 16.
    let at = |o: u32| {
        let o = (OUT + 4 + o) as usize;
        i16::from_le_bytes([memory.bytes[o], memory.bytes[o + 1]])
    };
    (at(14), at(16))
}

#[test]
fn game_rate_reads_see_mouse_motion_from_slice_rate_syncs() {
    let keyboard = KeyboardMouse::new(Bindings::default());
    let convert = ConvertConfig::default();
    let mut input = InputManager::new(convert).unwrap();
    input.add_backend(Box::new(KeyboardFallback::new(
        keyboard.clone(),
        None,
        convert,
    )));
    let mut config = Config::for_gamepad(0x100);
    config.cache_drive.globals = test_cache_globals();
    let mut xapi = Xapi::new(config, input, Utility::default()).unwrap();
    let mut memory = Memory::new();
    let mut thread = ThreadState::default();
    xapi.init_devices(&mut memory, 0, 0).unwrap();
    let handle = call(
        &mut xapi,
        &mut memory,
        &mut thread,
        Function::XInputOpen,
        &[0x100, 0, 0, 0],
    )
    .unwrap()
    .0;
    let mut reads = Vec::new();
    // Each step is one slice: a 125 Hz mouse moving right and up, a sync
    // after every slice, and a game frame every 200 slices that never lands
    // on a mouse event.
    for slice in 1..=1003u32 {
        if slice % 8 == 0 {
            keyboard.mouse_motion(1.0, -1.0);
        }
        sync(&mut xapi, &mut memory, &mut Signals::default()).unwrap();
        if slice % 200 == 3 && slice > 200 {
            call(
                &mut xapi,
                &mut memory,
                &mut thread,
                Function::XInputGetState,
                &[handle, OUT],
            )
            .unwrap();
            reads.push(thumb_r(&memory));
        }
    }
    assert_eq!(reads.len(), 5);
    for (i, (rx, ry)) in reads.into_iter().enumerate() {
        assert!(
            rx > 0 && ry > 0,
            "game frame {i} saw right stick ({rx}, {ry})"
        );
    }
}
