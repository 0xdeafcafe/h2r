use guest_layout::testing::FakeGuest;
use input::host::HostPadState;

use super::{Frame, PadSource, Recorded, Scripted};

fn forward() -> HostPadState {
    HostPadState {
        left_stick: (1.0, 0.0),
        ..Default::default()
    }
}

fn frame_at(n: u64, g: &FakeGuest) -> Frame<'_> {
    Frame {
        frame: n,
        tick: n as u32,
        machine: [7; 6],
        guest: g,
    }
}

#[test]
fn scripted_steps_play_in_turn_and_loop() {
    let g = FakeGuest::new();
    let mut s = Scripted::new(vec![(2, forward()), (1, HostPadState::default())], false);
    let seq: Vec<bool> = (10..14)
        .map(|n| s.pad(&frame_at(n, &g)) == forward())
        .collect();
    assert_eq!(seq, vec![true, true, false, false]);
    let mut s = Scripted::new(vec![(2, forward()), (1, HostPadState::default())], true);
    let seq: Vec<bool> = (0..6)
        .map(|n| s.pad(&frame_at(n, &g)) == forward())
        .collect();
    assert_eq!(seq, vec![true, true, false, true, true, false]);
}

#[test]
fn a_recording_plays_once_from_its_first_frame() {
    let g = FakeGuest::new();
    let mut r = Recorded::new(vec![forward(), HostPadState::default(), forward()]);
    let seq: Vec<bool> = (5..9)
        .map(|n| r.pad(&frame_at(n, &g)) == forward())
        .collect();
    assert_eq!(seq, vec![true, false, true, false]);
}

#[test]
fn a_closure_is_a_source() {
    let g = FakeGuest::new();
    let mut s = |f: &Frame<'_>| HostPadState {
        right_stick: (f.frame as f32 / 10.0, 0.0),
        ..Default::default()
    };
    let a = s.pad(&frame_at(1, &g));
    let b = s.pad(&frame_at(5, &g));
    assert!(a.right_stick.0 < b.right_stick.0);
}
