//! Synthetic reports through the stdcall XAPI adapter (no game data).
mod common;

use common::*;
use input::InputManager;
use input::backend::scripted::{ScriptedInput, Timeline};
use input::gamepad::{Capabilities, Feedback, Gamepad, Rumble, State};
use xapi::{Config, ThreadState, Xapi, adapter::Function};

struct Rig {
    xapi: Xapi<Utility>,
    memory: Memory,
    thread: ThreadState,
    script: ScriptedInput,
    signals: Signals,
    handle: u32,
}

impl Rig {
    fn new(text: &str, polling: Option<[u8; 4]>) -> Self {
        let script = Timeline::from_toml(text).unwrap().playback();
        let mut input = InputManager::new(Default::default()).unwrap();
        input.add_backend(Box::new(script.clone()));
        let mut rig = Self {
            xapi: Xapi::new(Config::for_gamepad(0x100), input, Utility::default()).unwrap(),
            memory: Memory::new(),
            thread: ThreadState { last_error: 123 },
            script,
            signals: Signals::default(),
            handle: 0,
        };
        rig.invoke(Function::XInitDevices, &[0, 0]);
        let polling = if let Some(bytes) = polling {
            rig.memory.put(0x300, &bytes);
            0x300
        } else {
            0
        };
        rig.handle = rig.invoke(Function::XInputOpen, &[0x100, 0, 0, polling]);
        assert_ne!(rig.handle, 0);
        rig
    }

    fn invoke(&mut self, function: Function, args: &[u32]) -> u32 {
        call(
            &mut self.xapi,
            &mut self.memory,
            &mut self.thread,
            function,
            args,
        )
        .unwrap()
        .0
    }

    fn advance(&mut self, ticks: u64) {
        self.script.advance_to(ticks).unwrap();
        sync(&mut self.xapi, &mut self.memory, &mut self.signals).unwrap();
    }

    fn state(&mut self) -> (u32, State) {
        let status = self.invoke(Function::XInputGetState, &[self.handle, 0x400]);
        let bytes = &self.memory.bytes[0x400..0x400 + State::SIZE];
        (
            status,
            State {
                packet_number: u32::from_le_bytes(bytes[..4].try_into().unwrap()),
                gamepad: Gamepad::from_bytes(bytes[4..].try_into().unwrap()),
            },
        )
    }

    fn changes(&mut self) -> (u32, u32, u32) {
        let changed = self.invoke(Function::XGetDeviceChanges, &[0x100, 0x500, 0x504]);
        (changed, self.memory.word(0x500), self.memory.word(0x504))
    }
}

#[test]
fn every_button_is_pressed_held_and_released_with_one_packet_per_change() {
    // Independent expected Xbox byte positions, not the host mapping table.
    for (name, digital, analog) in [
        ("dpad_up", 1, None),
        ("dpad_down", 2, None),
        ("dpad_left", 4, None),
        ("dpad_right", 8, None),
        ("start", 16, None),
        ("back", 32, None),
        ("left_thumb", 64, None),
        ("right_thumb", 128, None),
        ("a", 0, Some(0)),
        ("b", 0, Some(1)),
        ("x", 0, Some(2)),
        ("y", 0, Some(3)),
        ("black", 0, Some(4)),
        ("white", 0, Some(5)),
        ("left_trigger", 0, Some(6)),
        ("right_trigger", 0, Some(7)),
    ] {
        let mut rig = Rig::new(
            &format!("[[frame]]\nat_ticks=10\nbuttons=['{name}']\n[[frame]]\nat_ticks=20"),
            None,
        );
        assert_eq!(rig.state(), (0, State::default()));
        rig.advance(10);
        let mut expected = State {
            packet_number: 1,
            ..Default::default()
        };
        expected.gamepad.buttons.0 = digital;
        if let Some(i) = analog {
            expected.gamepad.analog[i] = 255;
        }
        assert_eq!(rig.state(), (0, expected), "press {name}");
        rig.advance(19);
        for _ in 0..3 {
            assert_eq!(rig.state(), (0, expected), "hold {name}");
        }
        rig.advance(20);
        assert_eq!(
            rig.state(),
            (
                0,
                State {
                    packet_number: 2,
                    ..Default::default()
                }
            ),
            "release {name}"
        );
        assert_eq!(rig.thread.last_error, 123);
    }
}

#[test]
fn all_pressure_bytes_and_stick_axes_reach_guest_without_an_extra_deadzone() {
    let mut rig = Rig::new(
        "[[frame]]\nat_ticks=0\nanalog=[1,32,64,96,128,160,192,255]\nleft_stick=[-32768,32767]\nright_stick=[32767,-32768]\n[[frame]]\nat_ticks=10\nleft_stick=[1,-1]\nright_stick=[-655,655]\n[[frame]]\nat_ticks=20",
        None,
    );
    let (_, state) = rig.state();
    assert_eq!(state.gamepad.analog, [1, 32, 64, 96, 128, 160, 192, 255]);
    assert_eq!(
        (
            state.gamepad.thumb_lx,
            state.gamepad.thumb_ly,
            state.gamepad.thumb_rx,
            state.gamepad.thumb_ry
        ),
        (-32768, 32767, 32767, -32768)
    );
    rig.advance(10);
    let (_, state) = rig.state();
    assert_eq!(state.gamepad.analog, [0; 8]);
    assert_eq!(
        (
            state.gamepad.thumb_lx,
            state.gamepad.thumb_ly,
            state.gamepad.thumb_rx,
            state.gamepad.thumb_ry
        ),
        (1, -1, -655, 655)
    );
    rig.advance(20);
    assert_eq!(rig.state().1.gamepad, Gamepad::default());
}

#[test]
fn every_pressure_value_is_preserved_through_xapi() {
    let text = (0..=255)
        .map(|v| format!("[[frame]]\nat_ticks={v}\nanalog=[{v},{v},{v},{v},{v},{v},{v},{v}]\n"))
        .collect::<String>();
    let mut rig = Rig::new(&text, None);
    for pressure in 0..=255 {
        rig.advance(pressure);
        let (status, state) = rig.state();
        assert_eq!(status, 0);
        assert_eq!(state.gamepad.analog, [pressure as u8; 8]);
        assert_eq!(state.packet_number, pressure as u32);
    }
}

#[test]
fn capabilities_advertise_all_controls_and_both_motors_in_packed_layout() {
    let mut rig = Rig::new("[[frame]]\nat_ticks=0", None);
    assert_eq!(
        rig.invoke(Function::XInputGetCapabilities, &[rig.handle, 0x600]),
        0
    );
    let bytes = &rig.memory.bytes[0x600..0x600 + Capabilities::SIZE];
    assert_eq!(
        bytes,
        &[
            1, 0, 0, 255, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
            255, 255, 255, 255, 255, 255
        ]
    );
    rig.advance(1);
    assert_eq!(
        rig.invoke(Function::XInputGetCapabilities, &[rig.handle, 0x600]),
        0
    );
    rig.invoke(Function::XInputClose, &[rig.handle]);
    assert_eq!(
        rig.invoke(Function::XInputGetCapabilities, &[rig.handle, 0x600]),
        6
    );
}

#[test]
fn manual_poll_publishes_only_on_explicit_xinput_poll() {
    let mut rig = Rig::new(
        "[[frame]]\nat_ticks=10\nbuttons=['a']\n[[frame]]\nat_ticks=20",
        Some([0, 8, 8, 0]),
    );
    rig.advance(10);
    assert_eq!(rig.state(), (0, State::default()));
    assert_eq!(rig.invoke(Function::XInputPoll, &[rig.handle]), 0);
    let pressed = rig.state();
    assert_eq!(pressed.1.gamepad.analog[0], 255);
    assert_eq!(pressed.1.packet_number, 1);
    rig.advance(20);
    assert_eq!(rig.state(), pressed);
    rig.invoke(Function::XInputPoll, &[rig.handle]);
    assert_eq!(
        rig.state(),
        (
            0,
            State {
                packet_number: 2,
                ..Default::default()
            }
        )
    );
}

#[test]
fn feedback_drives_both_motors_then_completes_and_the_buffer_can_be_reused() {
    let mut rig = Rig::new("[[frame]]\nat_ticks=0", None);
    for rumble in [
        Rumble {
            left_motor: 65535,
            right_motor: 1234,
        },
        Rumble::default(),
    ] {
        rig.memory.put(
            0x600,
            &Feedback {
                status: 42,
                h_event: 7,
                rumble,
            }
            .to_bytes(),
        );
        assert_eq!(
            rig.invoke(Function::XInputSetState, &[rig.handle, 0x600]),
            997
        );
        assert_eq!(rig.memory.word(0x600), 997);
        rig.advance(0);
        assert_eq!(rig.script.rumble(), rumble);
        assert_eq!(rig.memory.word(0x600), 0);
    }
    assert_eq!(rig.signals.calls, [7, 7]);
}

#[test]
fn device_changes_stale_handles_and_pending_feedback_follow_disconnect_reconnect() {
    let mut rig = Rig::new(
        "[[frame]]\nat_ticks=0\nbuttons=['a']\n[[frame]]\nat_ticks=10\nconnected=false\n[[frame]]\nat_ticks=20\nbuttons=['b']",
        None,
    );
    assert_eq!(rig.invoke(Function::XGetDevices, &[0x100]), 1);
    assert_eq!(rig.changes(), (1, 1, 0));
    let held = rig.state().1;
    rig.memory.put(
        0x600,
        &Feedback {
            status: 0,
            h_event: 9,
            rumble: Rumble {
                left_motor: 1,
                right_motor: 2,
            },
        }
        .to_bytes(),
    );
    rig.invoke(Function::XInputSetState, &[rig.handle, 0x600]);
    rig.advance(10);
    assert_eq!(rig.memory.word(0x600), 1167);
    assert_eq!(rig.signals.calls, [9]);
    assert_eq!(rig.state(), (1167, held), "retail disconnected snapshot");
    assert_eq!(rig.invoke(Function::XGetDevices, &[0x100]), 0);
    assert_eq!(
        rig.invoke(Function::XInputGetCapabilities, &[rig.handle, 0x700]),
        1167
    );
    rig.advance(20);
    assert_eq!(rig.changes(), (1, 1, 1), "reinsert reports both edges");
    assert_eq!(rig.changes(), (0, 0, 0));
    assert_eq!(rig.state(), (1167, held), "old handle stays stale");
    rig.handle = rig.invoke(Function::XInputOpen, &[0x100, 0, 0, 0]);
    assert_eq!(rig.state().1.gamepad.analog[..2], [0, 255]);
}

#[test]
fn skipped_disconnect_still_invalidates_the_guest_handle() {
    let mut rig = Rig::new(
        "[[frame]]\nat_ticks=10\nconnected=false\n[[frame]]\nat_ticks=20",
        None,
    );
    assert_eq!(rig.changes(), (1, 1, 0));
    rig.advance(30);
    assert_eq!(rig.changes(), (1, 1, 1));
    assert_eq!(rig.state().0, 1167);
}
